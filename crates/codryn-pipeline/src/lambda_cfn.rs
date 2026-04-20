//! Index AWS SAM / CloudFormation and Serverless Framework HTTP events as `Route` nodes.
//!
//! SAM/CFN supported subset: `AWS::Serverless::Function` with `Properties.Events` entries
//! whose `Type` is `HttpApi` or `Api`, with `Properties.Path` and `Properties.Method`.
//!
//! Serverless Framework v3/v4: `functions.<name>.events[]` with `httpApi` or `http` events.

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use std::path::Path;

const SAM_TEMPLATE_NAMES: &[&str] = &[
    "template.yaml",
    "template.yml",
    "sam.yaml",
    "sam.yml",
    "cloudformation.yaml",
    "cloudformation.yml",
];

const SLS_TEMPLATE_NAMES: &[&str] = &["serverless.yml", "serverless.yaml"];

/// Run after symbol extraction so handler `Function` QNs exist in the registry.
pub fn pass_lambda_cfn(
    buf: &mut GraphBuffer,
    _reg: &crate::registry::Registry,
    files: &[&DiscoveredFile],
    project: &str,
    repo_root: &Path,
) {
    let yaml_files: Vec<&DiscoveredFile> = files
        .iter()
        .copied()
        .filter(|f| f.language == Language::Yaml && is_sam_template_name(&f.rel_path))
        .collect();

    if yaml_files.is_empty() {
        return;
    }

    for f in yaml_files {
        let text = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let doc: serde_yaml::Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(path = %f.rel_path, error = %e, "lambda_cfn: skip invalid YAML");
                continue;
            }
        };
        let template_dir = f.abs_path.parent().unwrap_or(repo_root);
        let globals_fn = doc
            .get("Globals")
            .and_then(|g| g.get("Function"))
            .cloned()
            .unwrap_or(serde_yaml::Value::Null);

        let resources = match doc.get("Resources").and_then(|r| r.as_mapping()) {
            Some(m) => m,
            None => continue,
        };

        for (_res_id, res_body) in resources {
            let res_body = match res_body.as_mapping() {
                Some(m) => m,
                None => continue,
            };
            let ty = res_body
                .get(serde_yaml::Value::String("Type".into()))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            if ty == "AWS::Serverless::Function" || ty == "AWS::Lambda::Function" {
                let mut props = res_body
                    .get(serde_yaml::Value::String("Properties".into()))
                    .cloned()
                    .unwrap_or(serde_yaml::Value::Mapping(Default::default()));
                merge_globals_function_props(&mut props, &globals_fn);
                emit_routes_for_function(buf, project, repo_root, template_dir, &props);
            }
        }
    }
}

fn is_sam_template_name(rel_path: &str) -> bool {
    let name = Path::new(rel_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    SAM_TEMPLATE_NAMES.contains(&name)
}

fn merge_globals_function_props(props: &mut serde_yaml::Value, globals_fn: &serde_yaml::Value) {
    let Some(gm) = globals_fn.as_mapping() else {
        return;
    };
    let Some(pm) = props.as_mapping_mut() else {
        return;
    };
    for (k, v) in gm {
        if !pm.contains_key(k) {
            pm.insert(k.clone(), v.clone());
        }
    }
}

fn emit_routes_for_function(
    buf: &mut GraphBuffer,
    project: &str,
    repo_root: &Path,
    template_dir: &Path,
    props: &serde_yaml::Value,
) {
    let Some(pm) = props.as_mapping() else {
        return;
    };
    let code_uri = pm
        .get(serde_yaml::Value::String("CodeUri".into()))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let handler_str = pm
        .get(serde_yaml::Value::String("Handler".into()))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if handler_str.is_empty() {
        return;
    }

    let handler_rel = match resolve_handler_file_rel(repo_root, template_dir, code_uri, handler_str)
    {
        Some(p) => p,
        None => {
            tracing::debug!(handler = %handler_str, "lambda_cfn: could not resolve handler file");
            return;
        }
    };
    let (file_stem_path, export_name) = split_handler(handler_str);
    let _ = file_stem_path; // used only in resolve

    let handler_qn = fqn::fqn_compute(project, &handler_rel, Some(export_name));

    let events = match pm.get(serde_yaml::Value::String("Events".into())) {
        Some(serde_yaml::Value::Mapping(em)) => em,
        _ => return,
    };

    for (_ev_name, ev_body) in events {
        let Some(ev_map) = ev_body.as_mapping() else {
            continue;
        };
        let ev_type = ev_map
            .get(serde_yaml::Value::String("Type".into()))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if !matches_http_event_type(ev_type) {
            continue;
        }
        let ev_props = ev_map
            .get(serde_yaml::Value::String("Properties".into()))
            .and_then(|p| p.as_mapping())
            .cloned()
            .unwrap_or_default();

        let path = ev_props
            .get(serde_yaml::Value::String("Path".into()))
            .and_then(|p| p.as_str())
            .unwrap_or("/");
        let path = normalize_http_path(path);
        let method = ev_props
            .get(serde_yaml::Value::String("Method".into()))
            .and_then(|m| m.as_str())
            .unwrap_or("GET")
            .to_uppercase();

        let route_qn = format!(
            "{project}.lambda.route.{method}.{}",
            path_to_qn_segment(&path)
        );
        let display_name = format!("{method} {path}");
        let props_json = serde_json::json!({
            "http_method": method,
            "path": path,
            "method_name": export_name,
            "source": "lambda"
        })
        .to_string();

        buf.add_node(
            "Route",
            &display_name,
            &route_qn,
            &handler_rel,
            1,
            1,
            Some(props_json),
        );
        buf.add_edge_by_qn(&handler_qn, &route_qn, "HANDLES_ROUTE", None);
    }
}

fn matches_http_event_type(t: &str) -> bool {
    matches!(t, "HttpApi" | "Api" | "Http" | "RestApi" | "ApiGatewayHttp") || t.contains("HttpApi")
}

fn split_handler(handler: &str) -> (&str, &str) {
    if let Some(pos) = handler.rfind('.') {
        (&handler[..pos], &handler[pos + 1..])
    } else {
        (handler, "handler")
    }
}

/// Resolve repo-relative path to the handler module file (first existing extension).
fn resolve_handler_file_rel(
    repo_root: &Path,
    template_dir: &Path,
    code_uri: &str,
    handler: &str,
) -> Option<String> {
    let (file_stem, _) = split_handler(handler);
    let base = template_dir.join(code_uri.trim_end_matches('/'));
    let file_path = base.join(file_stem);
    // Try exact path as file (no extension in stem)
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        let candidate = if file_path.extension().is_some() {
            file_path.clone()
        } else {
            file_path.with_extension(ext)
        };
        if candidate.is_file() {
            let rel = path_relative_to_repo(repo_root, &candidate)?;
            return Some(rel);
        }
    }
    None
}

fn path_relative_to_repo(repo_root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(repo_root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

fn normalize_http_path(p: &str) -> String {
    let p = p.trim();
    if p.is_empty() || p == "/" {
        return "/".into();
    }
    let p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("/{p}")
    };
    p
}

fn path_to_qn_segment(path: &str) -> String {
    path.trim_start_matches('/')
        .replace('/', "_")
        .replace(['{', '}', '*'], "_")
}

/// Index Serverless Framework v3/v4 `serverless.yml` / `serverless.yaml` files.
///
/// Emits `Route` nodes (source = `"serverless"`) for every `httpApi` / `http` event
/// and links them to the handler function via `HANDLES_ROUTE`.
pub fn pass_serverless_sls(
    buf: &mut GraphBuffer,
    files: &[&DiscoveredFile],
    project: &str,
    repo_root: &Path,
) {
    let sls_files: Vec<&DiscoveredFile> = files
        .iter()
        .copied()
        .filter(|f| f.language == Language::Yaml && is_sls_template(&f.rel_path))
        .collect();

    for f in sls_files {
        let text = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let doc: serde_yaml::Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(path = %f.rel_path, error = %e, "serverless_sls: skip invalid YAML");
                continue;
            }
        };

        // Must have a top-level `functions` mapping — distinguishes SLS from arbitrary YAML.
        let Some(functions) = doc.get("functions").and_then(|v| v.as_mapping()) else {
            continue;
        };

        // Skip non-AWS providers (Azure Functions, GCP) — they use different runtimes.
        if let Some(provider_name) = doc
            .get("provider")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
        {
            if provider_name != "aws" {
                tracing::debug!(provider = %provider_name, "serverless_sls: skip non-AWS provider");
                continue;
            }
        }

        let template_dir = f.abs_path.parent().unwrap_or(repo_root);

        for (_fn_key, fn_body) in functions {
            let Some(fn_map) = fn_body.as_mapping() else {
                continue;
            };

            let handler_str = fn_map
                .get(serde_yaml::Value::String("handler".into()))
                .and_then(|h| h.as_str())
                .unwrap_or("");
            if handler_str.is_empty() {
                continue;
            }

            let Some(events) = fn_map
                .get(serde_yaml::Value::String("events".into()))
                .and_then(|e| e.as_sequence())
            else {
                continue;
            };

            // Resolve handler source file (no separate CodeUri in SLS — handler path is direct).
            let handler_rel = match resolve_handler_file_rel(
                repo_root,
                template_dir,
                "",
                handler_str,
            ) {
                Some(r) => r,
                None => {
                    tracing::debug!(handler = %handler_str, "serverless_sls: could not resolve handler file");
                    continue;
                }
            };
            let (_, export_name) = split_handler(handler_str);
            let handler_qn = fqn::fqn_compute(project, &handler_rel, Some(export_name));

            for event in events {
                let Some(ev_map) = event.as_mapping() else {
                    continue;
                };

                // SLS HTTP event keys: `httpApi` (v2 / HTTP API) and `http` (v1 / REST API).
                for event_key in ["httpApi", "http"] {
                    let key = serde_yaml::Value::String(event_key.into());
                    let Some(ev_props) = ev_map.get(&key) else {
                        continue;
                    };

                    let (path, method) = parse_sls_http_event(ev_props);
                    let path = normalize_http_path(&path);
                    let method = method.to_uppercase();

                    let route_qn = format!(
                        "{project}.serverless.route.{method}.{}",
                        path_to_qn_segment(&path)
                    );
                    let props_json = serde_json::json!({
                        "http_method": method,
                        "path": path,
                        "method_name": export_name,
                        "source": "serverless",
                    })
                    .to_string();
                    buf.add_node(
                        "Route",
                        &format!("{method} {path}"),
                        &route_qn,
                        &handler_rel,
                        1,
                        1,
                        Some(props_json),
                    );
                    buf.add_edge_by_qn(&handler_qn, &route_qn, "HANDLES_ROUTE", None);
                }
            }
        }
    }
}

/// Parse a Serverless Framework HTTP event value.
///
/// Supports three forms:
/// - Mapping: `{ path: /foo, method: GET }`
/// - Short string: `"GET /foo"` (v3+ shorthand)
/// - String with only path: `"/foo"` → method = GET
fn parse_sls_http_event(ev: &serde_yaml::Value) -> (String, String) {
    match ev {
        serde_yaml::Value::Mapping(m) => {
            let path = m
                .get(serde_yaml::Value::String("path".into()))
                .and_then(|p| p.as_str())
                .unwrap_or("/")
                .to_string();
            let method = m
                .get(serde_yaml::Value::String("method".into()))
                .and_then(|m| m.as_str())
                .unwrap_or("GET")
                .to_string();
            (path, method)
        }
        serde_yaml::Value::String(s) => {
            let s = s.trim();
            let parts: Vec<&str> = s.splitn(2, ' ').collect();
            if parts.len() == 2 {
                // "GET /path" or "POST /path"
                (parts[1].to_string(), parts[0].to_string())
            } else {
                // Just a path
                (s.to_string(), "GET".into())
            }
        }
        _ => ("/".into(), "GET".into()),
    }
}

fn is_sls_template(rel_path: &str) -> bool {
    let name = Path::new(rel_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    SLS_TEMPLATE_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction;
    use crate::registry::Registry;
    use codryn_discover::{DiscoveredFile, Language};
    use codryn_graph_buffer::GraphBuffer;

    #[test]
    fn sam_yaml_emits_route_and_edge() {
        let dir = std::env::temp_dir().join(format!("codryn_lam_{}", std::process::id() as u32));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("template.yaml"),
            r#"Resources:
  HelloFn:
    Type: AWS::Serverless::Function
    Properties:
      CodeUri: src/
      Handler: handler.main
      Events:
        GetHello:
          Type: HttpApi
          Properties:
            Path: /hello
            Method: GET
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("src/handler.ts"),
            "export async function main() { return {}; }\n",
        )
        .unwrap();

        let yaml_f = DiscoveredFile {
            abs_path: dir.join("template.yaml"),
            rel_path: "template.yaml".into(),
            language: Language::Yaml,
        };
        let ts_f = DiscoveredFile {
            abs_path: dir.join("src/handler.ts"),
            rel_path: "src/handler.ts".into(),
            language: Language::TypeScript,
        };
        let mut reg = Registry::new();
        extraction::register_file(&mut reg, "p", &ts_f);
        let mut buf = GraphBuffer::new("p");
        pass_lambda_cfn(&mut buf, &reg, &[&yaml_f], "p", &dir);
        assert!(buf.node_count() >= 1, "expected Route node");
        assert!(buf.edge_count() >= 1, "expected HANDLES_ROUTE edge");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_handler_basic() {
        assert_eq!(split_handler("index.handler"), ("index", "handler"));
        assert_eq!(
            split_handler("src/handlers/foo.run"),
            ("src/handlers/foo", "run")
        );
    }

    #[test]
    fn path_to_qn() {
        assert_eq!(path_to_qn_segment("/users"), "users");
        assert_eq!(path_to_qn_segment("/api/v1/items"), "api_v1_items");
    }

    #[test]
    fn serverless_yml_mapping_form() {
        let dir = std::env::temp_dir().join(format!("codryn_sls_{}", std::process::id() as u32));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/functions")).unwrap();
        std::fs::write(
            dir.join("serverless.yml"),
            r#"service: my-service
provider:
  name: aws
  runtime: nodejs18.x
functions:
  createBooking:
    handler: src/functions/createBooking.handler
    events:
      - httpApi:
          path: /bookings
          method: POST
  getBookings:
    handler: src/functions/getBookings.handler
    events:
      - httpApi:
          path: /bookings
          method: GET
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("src/functions/createBooking.ts"),
            "export async function handler() { return {}; }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/functions/getBookings.ts"),
            "export async function handler() { return []; }\n",
        )
        .unwrap();

        let sls_f = DiscoveredFile {
            abs_path: dir.join("serverless.yml"),
            rel_path: "serverless.yml".into(),
            language: Language::Yaml,
        };
        let ts_create = DiscoveredFile {
            abs_path: dir.join("src/functions/createBooking.ts"),
            rel_path: "src/functions/createBooking.ts".into(),
            language: codryn_discover::Language::TypeScript,
        };
        let ts_get = DiscoveredFile {
            abs_path: dir.join("src/functions/getBookings.ts"),
            rel_path: "src/functions/getBookings.ts".into(),
            language: codryn_discover::Language::TypeScript,
        };
        let mut reg = Registry::new();
        extraction::register_file(&mut reg, "p", &ts_create);
        extraction::register_file(&mut reg, "p", &ts_get);
        let mut buf = GraphBuffer::new("p");
        pass_serverless_sls(&mut buf, &[&sls_f, &ts_create, &ts_get], "p", &dir);
        assert!(
            buf.node_count() >= 2,
            "expected 2 Route nodes, got {}",
            buf.node_count()
        );
        assert!(buf.edge_count() >= 2, "expected 2 HANDLES_ROUTE edges");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serverless_yml_shorthand_form() {
        let dir =
            std::env::temp_dir().join(format!("codryn_sls_short_{}", std::process::id() as u32));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("serverless.yml"),
            r#"service: my-service
provider:
  name: aws
functions:
  hello:
    handler: handler.main
    events:
      - httpApi: 'GET /hello'
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("handler.ts"),
            "export async function main() { return 'hi'; }\n",
        )
        .unwrap();

        let sls_f = DiscoveredFile {
            abs_path: dir.join("serverless.yml"),
            rel_path: "serverless.yml".into(),
            language: Language::Yaml,
        };
        let ts_f = DiscoveredFile {
            abs_path: dir.join("handler.ts"),
            rel_path: "handler.ts".into(),
            language: codryn_discover::Language::TypeScript,
        };
        let mut buf = GraphBuffer::new("p");
        pass_serverless_sls(&mut buf, &[&sls_f, &ts_f], "p", &dir);
        assert!(
            buf.node_count() >= 1,
            "expected Route node from shorthand httpApi event"
        );
        assert!(buf.edge_count() >= 1, "expected HANDLES_ROUTE edge");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn serverless_yml_skips_non_aws() {
        let dir =
            std::env::temp_dir().join(format!("codryn_sls_gcp_{}", std::process::id() as u32));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("serverless.yml"),
            r#"service: my-service
provider:
  name: gcp
functions:
  hello:
    handler: handler.main
    events:
      - http:
          path: /hello
          method: GET
"#,
        )
        .unwrap();
        let sls_f = DiscoveredFile {
            abs_path: dir.join("serverless.yml"),
            rel_path: "serverless.yml".into(),
            language: Language::Yaml,
        };
        let mut buf = GraphBuffer::new("p");
        pass_serverless_sls(&mut buf, &[&sls_f], "p", &dir);
        assert_eq!(buf.node_count(), 0, "non-AWS provider should be skipped");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
