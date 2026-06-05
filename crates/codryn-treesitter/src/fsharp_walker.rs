//! F# regex-fallback walker.

use crate::TsSymbol;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_LET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^let\s+(?:rec\s+)?(\w+)").unwrap());
static RE_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^type\s+(\w+)").unwrap());
static RE_MODULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^module\s+(\w+)").unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_MODULE.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Module", ln));
        } else if let Some(cap) = RE_TYPE.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Class", ln));
        } else if let Some(cap) = RE_LET.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Function", ln));
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
