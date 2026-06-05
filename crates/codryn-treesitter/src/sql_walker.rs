//! SQL regex-fallback walker.

use crate::TsSymbol;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_TABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:`?(\w+)`?\.)?`?(\w+)`?").unwrap()
});
static RE_VIEW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)CREATE\s+(?:OR\s+REPLACE\s+)?(?:MATERIALIZED\s+)?VIEW\s+(?:`?(\w+)`?\.)?`?(\w+)`?",
    )
    .unwrap()
});
static RE_FUNC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+(?:OR\s+REPLACE\s+)?FUNCTION\s+(?:`?(\w+)`?\.)?`?(\w+)`?").unwrap()
});
static RE_PROC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)CREATE\s+(?:OR\s+REPLACE\s+)?PROCEDURE\s+(?:`?(\w+)`?\.)?`?(\w+)`?").unwrap()
});

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_TABLE.captures(line) {
            let name = last_group(&cap);
            symbols.push(sym(name, "Class", ln));
        } else if let Some(cap) = RE_VIEW.captures(line) {
            let name = last_group(&cap);
            symbols.push(sym(name, "Class", ln));
        } else if let Some(cap) = RE_FUNC.captures(line) {
            let name = last_group(&cap);
            symbols.push(sym(name, "Function", ln));
        } else if let Some(cap) = RE_PROC.captures(line) {
            let name = last_group(&cap);
            symbols.push(sym(name, "Function", ln));
        }
    }
    symbols
}

fn last_group(cap: &regex::Captures) -> String {
    // The name is in group 2 (group 1 is optional schema)
    cap.get(2)
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
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
