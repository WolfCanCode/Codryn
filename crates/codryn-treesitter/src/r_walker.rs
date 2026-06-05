//! R regex-fallback walker.

use crate::{TsParam, TsSymbol};
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_FUNC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\w[\w.]*)\s*(?:<-|=)\s*function\s*\(([^)]*)\)").unwrap());
static RE_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^setClass\s*\(\s*"(\w+)""#).unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_FUNC.captures(line) {
            let params = parse_params(&cap[2]);
            symbols.push(sym(cap[1].to_string(), "Function", ln, params));
        } else if let Some(cap) = RE_CLASS.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Class", ln, vec![]));
        }
    }
    symbols
}

fn parse_params(s: &str) -> Vec<TsParam> {
    s.split(',')
        .filter_map(|p| {
            let name = p.split('=').next()?.trim().to_string();
            if name.is_empty() {
                None
            } else {
                Some(TsParam {
                    name,
                    type_name: None,
                })
            }
        })
        .collect()
}

fn sym(name: String, label: &str, line: i32, params: Vec<TsParam>) -> TsSymbol {
    TsSymbol {
        name,
        label: label.into(),
        start_line: line,
        end_line: line,
        parent_name: None,
        signature: None,
        return_type: None,
        parameters: params,
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
