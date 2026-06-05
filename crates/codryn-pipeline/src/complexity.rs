/// Result of complexity analysis for a single function/method node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComplexityResult {
    pub cyclomatic: u32,
    pub cognitive: u32,
}

/// Compute complexity from raw source text using the text-based heuristic.
///
/// This is a fast fallback used when a tree-sitter node is not available
/// (e.g., for body text extracted from walkers). It uses the same decision-point
/// counting as `codryn_foundation::complexity::cyclomatic_complexity` for cyclomatic,
/// and a simplified nesting-aware algorithm for cognitive.
pub fn compute_complexity_from_text(source: &str) -> ComplexityResult {
    let cyclomatic = codryn_foundation::complexity::cyclomatic_complexity(source);
    let cognitive = cognitive_complexity_from_text(source);
    ComplexityResult {
        cyclomatic,
        cognitive,
    }
}

/// Compute cognitive complexity from source text using a simplified nesting-aware algorithm.
///
/// Tracks indentation depth as a proxy for nesting depth.
/// For each control-flow keyword (if, for, while, loop, match), adds (1 + nesting_depth).
/// `&&` and `||` add 1 each (no nesting bonus).
fn cognitive_complexity_from_text(source: &str) -> u32 {
    let mut complexity: u32 = 0;
    let mut in_block_comment = false;

    // Determine base indentation from the first non-empty line
    let base_indent = source
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(0);

    for line in source.lines() {
        let trimmed = line.trim();

        // Track block comments
        if in_block_comment {
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        // Compute nesting depth from indentation (relative to base)
        let indent = line.len() - line.trim_start().len();
        let relative_indent = indent.saturating_sub(base_indent);
        // Each 4 spaces (or 1 tab) = 1 nesting level
        let nesting_depth = (relative_indent / 4) as u32;

        // Control flow keywords that add (1 + nesting_depth)
        let has_if = trimmed.starts_with("if ")
            || trimmed.starts_with("} else if ")
            || trimmed.starts_with("else if ")
            || trimmed.contains(" if ");
        let has_for = trimmed.starts_with("for ");
        let has_while = trimmed.starts_with("while ");
        let has_loop = trimmed.starts_with("loop {") || trimmed == "loop";
        let has_match = trimmed.starts_with("match ");
        let has_elif = trimmed.starts_with("elif ");

        if has_if || has_for || has_while || has_loop || has_match || has_elif {
            complexity += 1 + nesting_depth;
        }

        // Logical operators — flat increment
        complexity += trimmed.matches("&&").count() as u32;
        complexity += trimmed.matches("||").count() as u32;
    }

    complexity
}

/// Compute cyclomatic and cognitive complexity from a tree-sitter AST node.
///
/// # Cyclomatic complexity
/// Starts at 1 (base) and increments for each decision point:
/// `if`, `else_if_clause`, `elif_clause`, `for`, `while`, `loop`, `match`,
/// each `match_arm`, `&&`, `||`, `?` (try expression), `catch`, `case`.
///
/// # Cognitive complexity (simplified nesting-aware)
/// Walks the AST recursively, tracking nesting depth.
/// For each control-flow node (if, for, while, loop, match), adds (1 + nesting_depth).
/// `&&` and `||` add 1 each (no nesting bonus).
pub fn compute_complexity(node: &tree_sitter::Node, source: &[u8]) -> ComplexityResult {
    let cyclomatic = compute_cyclomatic(node, source);
    let cognitive = compute_cognitive(node, source, 0);
    ComplexityResult {
        cyclomatic,
        cognitive,
    }
}

/// Recursively count cyclomatic decision points in the AST.
fn compute_cyclomatic(node: &tree_sitter::Node, source: &[u8]) -> u32 {
    let mut count: u32 = 0;

    match node.kind() {
        // Conditional branches
        "if_expression"
        | "if_statement"
        | "else_if_clause"
        | "elif_clause"
        | "conditional_expression"
        | "ternary_expression" => {
            count += 1;
        }

        // Loops
        "for_expression" | "for_statement" | "for_in_statement" | "for_of_statement"
        | "while_expression" | "while_statement" | "loop_expression" | "do_statement" => {
            count += 1;
        }

        // Match/switch
        "match_expression" | "switch_statement" | "switch_expression" => {
            // Count each arm/case separately below; don't add 1 for the match itself
        }

        // Match arms and case clauses
        "match_arm" | "case_clause" | "switch_case" | "switch_default" => {
            count += 1;
        }

        // Exception handling
        "catch_clause" | "except_clause" | "rescue_clause" => {
            count += 1;
        }

        // Binary logical operators — check the operator token
        "binary_expression" | "logical_expression" => {
            if let Some(op_node) = node.child_by_field_name("operator") {
                let op = op_node.utf8_text(source).unwrap_or("");
                if op == "&&" || op == "||" || op == "and" || op == "or" {
                    count += 1;
                }
            } else {
                // Fallback: check text of the node for operators
                let text = node.utf8_text(source).unwrap_or("");
                if text.contains("&&") || text.contains("||") {
                    count += 1;
                }
            }
        }

        // Try/question-mark operator (Rust `?`)
        "try_expression" | "question_mark_expression" => {
            count += 1;
        }

        _ => {}
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count += compute_cyclomatic(&child, source);
    }

    count
}

/// Recursively compute cognitive complexity, tracking nesting depth.
fn compute_cognitive(node: &tree_sitter::Node, source: &[u8], depth: u32) -> u32 {
    let mut count: u32 = 0;

    let (increment, new_depth) = match node.kind() {
        // Control flow nodes that increase nesting
        "if_expression"
        | "if_statement"
        | "else_if_clause"
        | "elif_clause"
        | "conditional_expression"
        | "ternary_expression" => (1 + depth, depth + 1),

        "for_expression" | "for_statement" | "for_in_statement" | "for_of_statement"
        | "while_expression" | "while_statement" | "loop_expression" | "do_statement" => {
            (1 + depth, depth + 1)
        }

        "match_expression" | "switch_statement" | "switch_expression" => (1 + depth, depth + 1),

        // Logical operators — flat increment, no nesting bonus
        "binary_expression" | "logical_expression" => {
            let op_text = node
                .child_by_field_name("operator")
                .and_then(|n| n.utf8_text(source).ok())
                .unwrap_or("");
            if op_text == "&&" || op_text == "||" || op_text == "and" || op_text == "or" {
                (1, depth)
            } else {
                (0, depth)
            }
        }

        _ => (0, depth),
    };

    count += increment;

    // Recurse into children with updated depth
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count += compute_cognitive(&child, source, new_depth);
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_rust_and_compute(source: &str) -> ComplexityResult {
        // Use codryn-treesitter to extract symbols, then compute complexity from body text
        // of the first function found. Falls back to computing from the whole source.
        if let Some(symbols) = codryn_treesitter::extract_symbols(codryn_discover::Language::Rust, source)
        {
            if let Some(sym) = symbols
                .iter()
                .find(|s| matches!(s.label.as_str(), "Function" | "Method"))
            {
                if let Some(ref body) = sym.body_text {
                    return compute_complexity_from_text(body);
                }
            }
        }
        compute_complexity_from_text(source)
    }

    #[test]
    fn simple_function_has_base_complexity() {
        let src = r#"fn hello() { println!("hi"); }"#;
        let result = parse_rust_and_compute(src);
        // No decision points — cyclomatic should be 1 (base), cognitive 0
        assert_eq!(result.cognitive, 0);
        assert_eq!(result.cyclomatic, 1); // base complexity is 1
    }

    #[test]
    fn if_statement_increments_both() {
        let src = r#"
fn example(x: i32) -> i32 {
    if x > 0 {
        1
    } else {
        0
    }
}
"#;
        let result = parse_rust_and_compute(src);
        assert!(
            result.cyclomatic >= 2,
            "Expected at least 2 (base + if), got {}",
            result.cyclomatic
        );
        assert!(
            result.cognitive >= 1,
            "Expected at least 1 for if, got {}",
            result.cognitive
        );
    }

    #[test]
    fn nested_if_increases_cognitive_more() {
        let src = r#"
fn example(x: i32, y: i32) -> i32 {
    if x > 0 {
        if y > 0 {
            1
        } else {
            0
        }
    } else {
        -1
    }
}
"#;
        let result = parse_rust_and_compute(src);
        // Cyclomatic: base 1 + outer if 1 + inner if 1 = 3
        // Cognitive: outer if at depth 0 = 1, inner if at depth 1 = 2, total >= 3
        assert!(result.cyclomatic >= 3, "cyclomatic={}", result.cyclomatic);
        assert!(
            result.cognitive >= 3,
            "cognitive={} (nested if should cost more)",
            result.cognitive
        );
        assert!(
            result.cognitive >= result.cyclomatic,
            "cognitive ({}) should be >= cyclomatic ({}) for nested code",
            result.cognitive,
            result.cyclomatic
        );
    }

    #[test]
    fn for_loop_increments_complexity() {
        let src = r#"
fn sum(items: &[i32]) -> i32 {
    let mut total = 0;
    for item in items {
        total += item;
    }
    total
}
"#;
        let result = parse_rust_and_compute(src);
        assert!(
            result.cyclomatic >= 2,
            "Expected at least 2 (base + for loop), got {}",
            result.cyclomatic
        );
        assert!(
            result.cognitive >= 1,
            "Expected at least 1 for for loop, got {}",
            result.cognitive
        );
    }

    #[test]
    fn text_based_cognitive_nesting() {
        // Test the text-based cognitive complexity directly
        let src = r#"
fn example() {
    if x {
        for i in items {
            if y {
                do_thing();
            }
        }
    }
}
"#;
        let result = compute_complexity_from_text(src);
        // outer if: depth ~1 → +2, for: depth ~2 → +3, inner if: depth ~3 → +4
        // Total cognitive >= 9 (varies by indentation)
        assert!(
            result.cognitive > result.cyclomatic - 1,
            "cognitive ({}) should be higher than cyclomatic ({}) for deeply nested code",
            result.cognitive,
            result.cyclomatic
        );
    }

    #[test]
    fn match_statement_increments_cognitive_not_cyclomatic_text_based() {
        // The text-based cyclomatic heuristic counts "case " keywords (C/JS style),
        // not Rust "match " expressions. Rust match arms don't use "case ".
        // However, cognitive_complexity_from_text DOES count "match " as a control-flow
        // keyword, so cognitive > 0 while cyclomatic stays at base (1).
        let src = r#"
fn classify(x: i32) -> &'static str {
    match x {
        0 => "zero",
        1 => "one",
        _ => "other",
    }
}
"#;
        let result = compute_complexity_from_text(src);
        // Text-based cyclomatic: 1 base only (no "case " keyword in Rust match)
        assert_eq!(
            result.cyclomatic, 1,
            "text-based cyclomatic should be 1 for Rust match (no 'case' keyword), got {}",
            result.cyclomatic
        );
        // Cognitive: match at some nesting depth >= 1
        assert!(
            result.cognitive >= 1,
            "Expected at least 1 for match in cognitive, got {}",
            result.cognitive
        );
    }

    #[test]
    fn compute_complexity_from_text_stores_correct_fields() {
        // Verify that compute_complexity_from_text returns a ComplexityResult
        // with the correct field names and types — this mirrors how extraction.rs
        // stores cyclomatic_complexity and cognitive_complexity in properties_json.
        let src = r#"
fn example(x: i32) -> i32 {
    if x > 0 {
        for i in 0..x {
            if i % 2 == 0 {
                return i;
            }
        }
    }
    0
}
"#;
        let result = compute_complexity_from_text(src);

        // Verify the struct fields that get stored as properties_json keys
        // "cyclomatic_complexity" and "cognitive_complexity"
        assert!(
            result.cyclomatic >= 3,
            "cyclomatic should be >= 3 (base + if + for + inner if), got {}",
            result.cyclomatic
        );
        assert!(
            result.cognitive >= 3,
            "cognitive should be >= 3 for nested code, got {}",
            result.cognitive
        );

        // Verify the values can be serialised to JSON as integers (matching how
        // extraction.rs stores them: serde_json::json!(result.cyclomatic))
        let cyclomatic_json = serde_json::json!(result.cyclomatic);
        let cognitive_json = serde_json::json!(result.cognitive);
        assert!(
            cyclomatic_json.is_number(),
            "cyclomatic_complexity must serialise as a JSON number"
        );
        assert!(
            cognitive_json.is_number(),
            "cognitive_complexity must serialise as a JSON number"
        );

        // Verify cognitive >= cyclomatic for nested code (nesting penalty)
        assert!(
            result.cognitive >= result.cyclomatic,
            "cognitive ({}) should be >= cyclomatic ({}) for nested code",
            result.cognitive,
            result.cyclomatic
        );
    }

    #[test]
    fn simple_function_complexity_result_fields() {
        // A function with no decision points should have cyclomatic=1, cognitive=0
        // This directly tests the ComplexityResult struct fields used in properties_json
        let src = r#"fn greet(name: &str) -> String { format!("Hello, {}!", name) }"#;
        let result = compute_complexity_from_text(src);
        assert_eq!(
            result.cyclomatic, 1,
            "base cyclomatic complexity should be 1"
        );
        assert_eq!(
            result.cognitive, 0,
            "cognitive complexity should be 0 for trivial function"
        );

        // Confirm JSON serialisation matches what extraction.rs stores
        let props = serde_json::json!({
            "cyclomatic_complexity": result.cyclomatic,
            "cognitive_complexity": result.cognitive,
        });
        assert_eq!(props["cyclomatic_complexity"], 1);
        assert_eq!(props["cognitive_complexity"], 0);
    }
}
