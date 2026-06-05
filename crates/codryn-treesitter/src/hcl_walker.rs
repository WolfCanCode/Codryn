//! HCL (Terraform) regex-fallback walker.

use crate::TsSymbol;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_RESOURCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(resource|data)\s+"([^"]+)"\s+"([^"]+)""#).unwrap());
static RE_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(variable|output|module)\s+"([^"]+)""#).unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_RESOURCE.captures(trimmed) {
            let _kind = &cap[1]; // resource or data
            let rtype = &cap[2];
            let rname = &cap[3];
            let name = format!("{}.{}", rtype, rname);
            let label = "Class";
            symbols.push(sym(name, label, ln));
        } else if let Some(cap) = RE_BLOCK.captures(trimmed) {
            let kind = &cap[1];
            let name = cap[2].to_string();
            let label = match kind {
                "module" => "Module",
                "output" => "Function",
                _ => "Class", // variable
            };
            symbols.push(sym(name, label, ln));
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
