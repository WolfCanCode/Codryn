//! Zig regex-fallback walker.

use crate::{TsParam, TsSymbol};
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_FN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:pub\s+)?fn\s+(\w+)\s*\(([^)]*)\)").unwrap());
static RE_STRUCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:pub\s+)?const\s+(\w+)\s*=\s*(?:packed\s+)?struct").unwrap());
static RE_ENUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:pub\s+)?const\s+(\w+)\s*=\s*enum").unwrap());
static RE_TEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^test\s+"([^"]+)""#).unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let ln = (i as i32) + 1;
        let is_pub = trimmed.starts_with("pub ");
        if let Some(cap) = RE_FN.captures(trimmed) {
            let params = parse_params(&cap[2]);
            symbols.push(sym(
                cap[1].to_string(),
                "Function",
                ln,
                params,
                is_pub,
                false,
            ));
        } else if let Some(cap) = RE_STRUCT.captures(trimmed) {
            symbols.push(sym(cap[1].to_string(), "Class", ln, vec![], is_pub, false));
        } else if let Some(cap) = RE_ENUM.captures(trimmed) {
            symbols.push(sym(cap[1].to_string(), "Class", ln, vec![], is_pub, false));
        } else if let Some(cap) = RE_TEST.captures(trimmed) {
            symbols.push(sym(cap[1].to_string(), "Function", ln, vec![], false, true));
        }
    }
    symbols
}

fn parse_params(s: &str) -> Vec<TsParam> {
    s.split(',')
        .filter_map(|p| {
            let name = p.split(':').next()?.trim().to_string();
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

fn sym(
    name: String,
    label: &str,
    line: i32,
    params: Vec<TsParam>,
    exported: bool,
    is_test: bool,
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
        is_exported: exported,
        is_abstract: false,
        is_async: false,
        is_test,
        is_entry_point: false,
        body_text: None,
    }
}
