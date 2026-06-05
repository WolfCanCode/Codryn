//! Protobuf regex-fallback walker.

use crate::TsSymbol;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_MESSAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^message\s+(\w+)").unwrap());
static RE_SERVICE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^service\s+(\w+)").unwrap());
static RE_RPC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*rpc\s+(\w+)").unwrap());
static RE_ENUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^enum\s+(\w+)").unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let mut current_service: Option<String> = None;
    for (i, line) in source.lines().enumerate() {
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_SERVICE.captures(line) {
            current_service = Some(cap[1].to_string());
            symbols.push(sym(cap[1].to_string(), "Interface", ln, None));
        } else if let Some(cap) = RE_MESSAGE.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Class", ln, None));
        } else if let Some(cap) = RE_ENUM.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Class", ln, None));
        } else if let Some(cap) = RE_RPC.captures(line) {
            symbols.push(sym(
                cap[1].to_string(),
                "Function",
                ln,
                current_service.clone(),
            ));
        }
        if line.trim_start().starts_with('}') && !line.trim_start().starts_with("}}") {
            // Rough heuristic: closing brace might end a service block
            // Not perfect but acceptable for regex fallback
        }
    }
    symbols
}

fn sym(name: String, label: &str, line: i32, parent: Option<String>) -> TsSymbol {
    TsSymbol {
        name,
        label: label.into(),
        start_line: line,
        end_line: line,
        parent_name: parent,
        signature: None,
        return_type: None,
        parameters: vec![],
        docstring: None,
        decorators: vec![],
        base_classes: vec![],
        is_exported: true,
        is_abstract: false,
        is_async: false,
        is_test: false,
        is_entry_point: false,
        body_text: None,
    }
}
