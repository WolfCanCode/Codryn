//! GraphQL regex-fallback walker.

use crate::TsSymbol;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:type|input|enum)\s+(\w+)").unwrap());
static RE_INTERFACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^interface\s+(\w+)").unwrap());
static RE_SCALAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^scalar\s+(\w+)").unwrap());
static RE_UNION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^union\s+(\w+)").unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_INTERFACE.captures(trimmed) {
            symbols.push(sym(cap[1].to_string(), "Interface", ln));
        } else if let Some(cap) = RE_TYPE.captures(trimmed) {
            symbols.push(sym(cap[1].to_string(), "Class", ln));
        } else if let Some(cap) = RE_SCALAR.captures(trimmed) {
            symbols.push(sym(cap[1].to_string(), "Class", ln));
        } else if let Some(cap) = RE_UNION.captures(trimmed) {
            symbols.push(sym(cap[1].to_string(), "Class", ln));
        }
    }
    symbols
}

fn sym(name: String, label: &str, line: i32) -> TsSymbol {
    TsSymbol {
        name,
        label: label.into(),
        start_line: line,
        end_line: line,
        parent_name: None,
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
