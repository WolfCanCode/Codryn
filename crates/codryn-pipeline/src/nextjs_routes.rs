//! Next.js Route Handlers (`app/**/route.ts`) and Pages Router API routes (`pages/api/**`).

use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use regex::Regex;
use std::sync::LazyLock;

use crate::registry::Registry;

static APP_ROUTE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(.+)/)?route\.(ts|tsx|js|jsx|mjs)$").unwrap()
});

static PAGES_API_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(.+\.(ts|tsx|js|jsx|mjs))$").unwrap()
});

static EXPORT_FN_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^export\s+(?:async\s+)?function\s+(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b")
        .unwrap()
});

static EXPORT_CONST_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^export\s+const\s+(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s*=")
        .unwrap()
});

/// Emit `Route` nodes for Next.js after the registry is populated.
pub fn pass_nextjs_routes(
    buf: &mut GraphBuffer,
    _reg: &Registry,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        if !matches!(
            f.language,
            Language::TypeScript | Language::Tsx | Language::JavaScript
        ) {
            continue;
        }
        let rel = f.rel_path.replace('\\', "/");
        if let Some(path) = app_router_path(&rel) {
            if let Ok(source) = std::fs::read_to_string(&f.abs_path) {
                for method in exported_http_methods(&source) {
                    emit_route(
                        buf,
                        project,
                        &f.rel_path,
                        &method,
                        &path,
                        "nextjs",
                    );
                }
            }
        } else if let Some(api_path) = pages_api_path(&rel) {
            emit_route(buf, project, &f.rel_path, "ANY", &api_path, "nextjs");
        }
    }
}

fn app_segment_start(path: &str) -> Option<usize> {
    if let Some(i) = path.find("/src/app/") {
        return Some(i + "/src/app/".len());
    }
    if let Some(i) = path.find("/app/") {
        return Some(i + "/app/".len());
    }
    if path.starts_with("app/") {
        return Some("app/".len());
    }
    None
}

fn app_router_path(rel_path: &str) -> Option<String> {
    let start = app_segment_start(rel_path)?;
    let after_app = &rel_path[start..];
    let caps = APP_ROUTE_SUFFIX.captures(after_app)?;
    let dir = caps.get(1).map(|m| m.as_str()).unwrap_or("");
    if dir.is_empty() {
        return Some("/".into());
    }
    let mut segments = Vec::new();
    for seg in dir.split('/') {
        if seg.is_empty() {
            continue;
        }
        if seg.starts_with('(') && seg.ends_with(')') {
            continue;
        }
        let part = if (seg.starts_with('[') && seg.ends_with(']'))
            || (seg.starts_with("[[...") && seg.ends_with("]]"))
        {
            let inner = seg
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim_start_matches('.')
                .trim_end_matches("...]");
            format!(":{inner}")
        } else {
            seg.to_string()
        };
        segments.push(part);
    }
    Some(format!("/{}", segments.join("/")).replace("//", "/"))
}

fn pages_segment_start(path: &str) -> Option<usize> {
    if let Some(i) = path.find("/src/pages/api/") {
        return Some(i + "/src/pages/api/".len());
    }
    if let Some(i) = path.find("/pages/api/") {
        return Some(i + "/pages/api/".len());
    }
    if path.starts_with("pages/api/") {
        return Some("pages/api/".len());
    }
    None
}

fn pages_api_path(rel_path: &str) -> Option<String> {
    let start = pages_segment_start(rel_path)?;
    let after = &rel_path[start..];
    let caps = PAGES_API_SUFFIX.captures(after)?;
    let file_part = caps.get(1)?.as_str();
    let stem = file_part
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(file_part);
    let url_tail = stem.replace('\\', "/");
    Some(format!("/api/{url_tail}"))
}

fn exported_http_methods(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in EXPORT_FN_METHOD.captures_iter(source) {
        if let Some(m) = c.get(1) {
            out.push(m.as_str().to_uppercase());
        }
    }
    for c in EXPORT_CONST_METHOD.captures_iter(source) {
        if let Some(m) = c.get(1) {
            out.push(m.as_str().to_uppercase());
        }
    }
    out.sort();
    out.dedup();
    if out.is_empty() {
        // Heuristic: file exists as route module but no explicit export yet
        out.push("GET".into());
    }
    out
}

fn emit_route(
    buf: &mut GraphBuffer,
    project: &str,
    file_rel: &str,
    method: &str,
    path: &str,
    source: &str,
) {
    let path_norm = if path == "/" {
        "/".into()
    } else {
        path.trim_end_matches('/').to_string()
    };
    let method_key = if method == "ANY" {
        "ANY"
    } else {
        method
    };
    let route_qn = format!(
        "{project}.next.route.{method_key}.{}",
        path_to_qn_segment(&path_norm)
    );
    let display = format!("{method_key} {path_norm}");
    let handler_name = if method == "ANY" {
        "handler"
    } else {
        method
    };
    let handler_qn = fqn::fqn_compute(project, file_rel, Some(handler_name));
    let props = serde_json::json!({
        "http_method": method_key,
        "path": path_norm,
        "method_name": handler_name,
        "source": source
    })
    .to_string();
    buf.add_node(
        "Route",
        &display,
        &route_qn,
        file_rel,
        1,
        1,
        Some(props),
    );
    buf.add_edge_by_qn(&handler_qn, &route_qn, "HANDLES_ROUTE", None);
}

fn path_to_qn_segment(path: &str) -> String {
    path.trim_start_matches('/')
        .replace('/', "_")
        .replace(['{', '}', '*'], "_")
        .replace(':', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_path_segments() {
        assert_eq!(
            app_router_path("src/app/api/users/route.ts").as_deref(),
            Some("/api/users")
        );
        assert_eq!(
            app_router_path("app/blog/route.ts").as_deref(),
            Some("/blog")
        );
        assert_eq!(
            app_router_path("app/(shop)/products/[id]/route.tsx").as_deref(),
            Some("/products/:id")
        );
    }

    #[test]
    fn pages_api() {
        assert_eq!(
            pages_api_path("src/pages/api/hello.ts").as_deref(),
            Some("/api/hello")
        );
    }
}
