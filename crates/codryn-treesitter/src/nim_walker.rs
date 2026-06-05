//! Nim regex-fallback walker.

use crate::{TsParam, TsSymbol};
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_FUNC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:proc|func|method|template|macro|iterator)\s+(\w+)\*?\s*(?:\[.*?\])?\s*\(([^)]*)\)",
    )
    .unwrap()
});
static RE_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+(\w+)\*?\s*=\s*(?:ref\s+)?object").unwrap());
static RE_TYPE_SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^type\b").unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    let mut in_type_section = false;
    for (i, line) in source.lines().enumerate() {
        let ln = (i as i32) + 1;
        if RE_TYPE_SECTION.is_match(line) {
            in_type_section = true;
            continue;
        }
        if in_type_section {
            if let Some(cap) = RE_TYPE.captures(line) {
                let name = cap[1].to_string();
                let exported = line.contains('*');
                symbols.push(sym(name, "Class", ln, vec![], exported));
            } else if !line.starts_with(' ') && !line.starts_with('\t') && !line.trim().is_empty() {
                in_type_section = false;
            }
        }
        if let Some(cap) = RE_FUNC.captures(line) {
            let name = cap[1].to_string();
            let exported = line.contains(&format!("{}*", &name));
            let params = parse_params(&cap[2]);
            symbols.push(sym(name, "Function", ln, params, exported));
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

fn sym(name: String, label: &str, line: i32, params: Vec<TsParam>, exported: bool) -> TsSymbol {
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
        is_test: false,
        is_entry_point: false,
        body_text: None,
    }
}
