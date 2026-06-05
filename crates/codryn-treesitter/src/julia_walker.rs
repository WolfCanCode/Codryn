//! Julia regex-fallback walker.

use crate::{TsParam, TsSymbol};
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_FUNC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:function|macro)\s+(\w+)\s*\(([^)]*)\)").unwrap());
static RE_STRUCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:mutable\s+)?struct\s+(\w+)").unwrap());
static RE_MODULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^module\s+(\w+)").unwrap());
static RE_ABSTRACT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^abstract\s+type\s+(\w+)").unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_FUNC.captures(trimmed) {
            let name = cap[1].to_string();
            let params_str = &cap[2];
            let params = parse_params(params_str);
            symbols.push(make_sym(
                name,
                "Function",
                ln,
                params,
                trimmed.starts_with("macro"),
            ));
        } else if let Some(cap) = RE_STRUCT.captures(trimmed) {
            symbols.push(make_sym(cap[1].to_string(), "Class", ln, vec![], false));
        } else if let Some(cap) = RE_MODULE.captures(trimmed) {
            symbols.push(make_sym(cap[1].to_string(), "Module", ln, vec![], false));
        } else if let Some(cap) = RE_ABSTRACT.captures(trimmed) {
            symbols.push(make_sym(cap[1].to_string(), "Interface", ln, vec![], false));
        }
    }
    symbols
}

fn parse_params(s: &str) -> Vec<TsParam> {
    s.split(',')
        .filter_map(|p| {
            let name = p.split("::").next()?.trim().to_string();
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

fn make_sym(
    name: String,
    label: &str,
    line: i32,
    params: Vec<TsParam>,
    _is_macro: bool,
) -> TsSymbol {
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
