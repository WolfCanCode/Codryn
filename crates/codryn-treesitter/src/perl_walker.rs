//! Perl regex-fallback walker.

use crate::TsSymbol;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_SUB: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^sub\s+(\w+)").unwrap());
static RE_PACKAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^package\s+([\w:]+)").unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_PACKAGE.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Module", ln));
        } else if let Some(cap) = RE_SUB.captures(line) {
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
