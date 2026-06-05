//! Clojure regex-fallback walker.

use crate::TsSymbol;
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::Tree;

static RE_DEFN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(defn-?\s+([\w\-/!?*+<>=]+)").unwrap());
static RE_DEFMACRO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(defmacro\s+([\w\-/!?*+<>=]+)").unwrap());
static RE_NS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(ns\s+([\w.\-]+)").unwrap());
static RE_DEFRECORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(defrecord\s+(\w+)").unwrap());
static RE_DEFPROTOCOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(defprotocol\s+(\w+)").unwrap());

pub fn walk_tree(_tree: &Tree, source: &str) -> Vec<TsSymbol> {
    let mut symbols = Vec::new();
    for (i, line) in source.lines().enumerate() {
        let ln = (i as i32) + 1;
        if let Some(cap) = RE_NS.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Module", ln));
        }
        if let Some(cap) = RE_DEFPROTOCOL.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Interface", ln));
        } else if let Some(cap) = RE_DEFRECORD.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Class", ln));
        } else if let Some(cap) = RE_DEFMACRO.captures(line) {
            symbols.push(sym(cap[1].to_string(), "Function", ln));
        } else if let Some(cap) = RE_DEFN.captures(line) {
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
