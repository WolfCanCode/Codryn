//! Documentation coverage detection for extracted symbols.
//!
//! Provides text-based detection of doc comments preceding symbol definitions.
//! Used as a fallback when tree-sitter walkers don't populate the `docstring` field.

/// Check if a symbol has a doc comment by looking at the source text
/// immediately preceding the symbol's start line.
/// Detects: ///, /**, #, """ (Python docstrings)
pub fn has_doc_comment_from_text(source: &str, start_line: i32) -> bool {
    doc_comment_lines_from_text(source, start_line) > 0
}

/// Count the number of doc comment lines preceding a symbol.
///
/// Scans backwards from the line immediately before `start_line`, counting
/// consecutive doc comment lines. Stops at the first non-comment, non-blank line.
/// A single blank line between the doc comment and the symbol is tolerated only
/// if doc lines have already been found (i.e., blank lines before any doc comment
/// are skipped, but a blank line after finding doc lines terminates the scan).
pub fn doc_comment_lines_from_text(source: &str, start_line: i32) -> u32 {
    let lines: Vec<&str> = source.lines().collect();
    // start_line is 1-indexed; the line before it is at index (start_line - 2)
    let start_idx = (start_line - 1).max(0) as usize;

    let mut count = 0u32;
    let mut i = start_idx;

    // Scan backwards from the line before start_line
    while i > 0 {
        i -= 1;
        let trimmed = lines[i].trim();

        if is_doc_comment_line(trimmed) {
            count += 1;
        } else if trimmed.is_empty() {
            // Allow one blank line between doc comment and symbol
            if count > 0 {
                break; // Already found some doc lines, stop at blank
            }
            // Continue scanning if we haven't found any yet
        } else {
            break; // Non-comment, non-blank line — stop
        }
    }

    count
}

/// Returns true if the trimmed line looks like a doc comment line.
fn is_doc_comment_line(trimmed: &str) -> bool {
    trimmed.starts_with("///")
        || trimmed.starts_with("/**")
        || trimmed.starts_with(" * ")
        || trimmed.starts_with("*/")
        || trimmed.starts_with("* ")
        || trimmed == "*"
        || trimmed.starts_with('#') // Python/Ruby/Shell
        || trimmed.starts_with("\"\"\"") // Python docstring
        || trimmed.starts_with("'''") // Python docstring
        || trimmed.starts_with("--") // Haskell/SQL
        || trimmed.starts_with(";;") // Lisp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_triple_slash_doc_comment() {
        let source = r#"
/// This is a doc comment.
/// It spans multiple lines.
fn my_function() {}
"#;
        // my_function is on line 4 (1-indexed)
        assert!(has_doc_comment_from_text(source, 4));
        assert_eq!(doc_comment_lines_from_text(source, 4), 2);
    }

    #[test]
    fn no_doc_comment() {
        let source = r#"
fn my_function() {}
"#;
        assert!(!has_doc_comment_from_text(source, 2));
        assert_eq!(doc_comment_lines_from_text(source, 2), 0);
    }

    #[test]
    fn javadoc_style_comment() {
        let source = r#"
/**
 * This is a Javadoc comment.
 * @param x the value
 */
public void myMethod() {}
"#;
        // myMethod is on line 6
        assert!(has_doc_comment_from_text(source, 6));
        assert!(doc_comment_lines_from_text(source, 6) >= 3);
    }

    #[test]
    fn python_hash_comment() {
        let source = "# This is a Python doc comment\ndef my_func():\n    pass\n";
        // my_func is on line 2
        assert!(has_doc_comment_from_text(source, 2));
        assert_eq!(doc_comment_lines_from_text(source, 2), 1);
    }

    #[test]
    fn python_triple_quote_docstring() {
        let source = r#"
"""
Module-level docstring.
"""
def my_func():
    pass
"#;
        // my_func is on line 5
        assert!(has_doc_comment_from_text(source, 5));
    }

    #[test]
    fn blank_line_between_comment_and_symbol_stops_scan() {
        let source = r#"
/// Doc comment
fn first() {}

fn second() {}
"#;
        // second() is on line 5 — blank line separates it from the doc comment
        // The blank line is encountered after finding 0 doc lines, so we keep scanning
        // but then hit the closing brace of first() which is not a doc comment → stop
        assert!(!has_doc_comment_from_text(source, 5));
    }

    #[test]
    fn blank_line_after_doc_lines_stops_scan() {
        let source = "/// Doc comment\n\nfn my_func() {}\n";
        // my_func is on line 3; blank line is on line 2, doc comment on line 1
        // Scan: line 2 is blank (count=0, continue), line 1 is doc (count=1), done
        assert!(has_doc_comment_from_text(source, 3));
        assert_eq!(doc_comment_lines_from_text(source, 3), 1);
    }

    #[test]
    fn first_line_symbol_has_no_preceding_lines() {
        let source = "fn my_func() {}\n";
        assert!(!has_doc_comment_from_text(source, 1));
        assert_eq!(doc_comment_lines_from_text(source, 1), 0);
    }

    // ── Additional doc comment style tests ───────────────────────────────────

    #[test]
    fn single_line_triple_slash() {
        let source = "/// Single line doc.\nfn foo() {}\n";
        assert!(has_doc_comment_from_text(source, 2));
        assert_eq!(doc_comment_lines_from_text(source, 2), 1);
    }

    #[test]
    fn haskell_sql_double_dash_comment() {
        let source = "-- | Haskell doc comment\nfoo :: Int -> Int\n";
        assert!(has_doc_comment_from_text(source, 2));
        assert_eq!(doc_comment_lines_from_text(source, 2), 1);
    }

    #[test]
    fn lisp_semicolon_comment() {
        let source = ";; This is a Lisp doc comment\n(defun my-func () nil)\n";
        assert!(has_doc_comment_from_text(source, 2));
        assert_eq!(doc_comment_lines_from_text(source, 2), 1);
    }

    #[test]
    fn python_single_quote_docstring() {
        let source = "'''\nModule docstring.\n'''\ndef my_func():\n    pass\n";
        // my_func is on line 4
        assert!(has_doc_comment_from_text(source, 4));
    }

    #[test]
    fn javadoc_multiline_star_lines() {
        let source = "/**\n * Line one.\n * Line two.\n */\nvoid method() {}\n";
        // method is on line 5
        assert!(has_doc_comment_from_text(source, 5));
        assert!(doc_comment_lines_from_text(source, 5) >= 3);
    }

    #[test]
    fn non_doc_regular_comment_not_detected() {
        // A regular // comment (not ///) should NOT be detected as a doc comment
        let source = "// regular comment\nfn foo() {}\n";
        assert!(!has_doc_comment_from_text(source, 2));
    }

    #[test]
    fn multiple_hash_lines() {
        let source = "# Line 1\n# Line 2\n# Line 3\ndef my_func():\n    pass\n";
        // my_func is on line 4
        assert!(has_doc_comment_from_text(source, 4));
        assert_eq!(doc_comment_lines_from_text(source, 4), 3);
    }

    // ── Coverage percentage calculation tests ────────────────────────────────

    /// Helper: compute coverage percentage the same way query_doc_coverage does.
    fn coverage_pct(documented: u32, total: u32) -> f64 {
        if total == 0 {
            0.0
        } else {
            documented as f64 / total as f64 * 100.0
        }
    }

    #[test]
    fn coverage_pct_all_documented() {
        assert!((coverage_pct(5, 5) - 100.0).abs() < 0.001);
    }

    #[test]
    fn coverage_pct_none_documented() {
        assert!((coverage_pct(0, 10) - 0.0).abs() < 0.001);
    }

    #[test]
    fn coverage_pct_half_documented() {
        assert!((coverage_pct(5, 10) - 50.0).abs() < 0.001);
    }

    #[test]
    fn coverage_pct_zero_total_returns_zero() {
        assert!((coverage_pct(0, 0) - 0.0).abs() < 0.001);
    }

    #[test]
    fn needs_attention_below_50_percent() {
        // A module with < 50% coverage needs attention
        let pct = coverage_pct(3, 10); // 30%
        assert!(pct < 50.0);
    }

    #[test]
    fn needs_attention_exactly_50_percent_does_not_flag() {
        // Exactly 50% should NOT need attention (threshold is strictly < 50)
        let pct = coverage_pct(5, 10); // 50%
        assert!(pct >= 50.0);
    }

    #[test]
    fn needs_attention_above_50_percent_does_not_flag() {
        let pct = coverage_pct(8, 10); // 80%
        assert!(pct >= 50.0);
    }

    // ── Module grouping (text-based) ─────────────────────────────────────────

    /// Verify that symbols in different "modules" (files) can be independently
    /// assessed for doc coverage using the text-based detection.
    #[test]
    fn module_a_has_docs_module_b_does_not() {
        // Simulate two files: module_a has a doc comment, module_b does not.
        let module_a_source = "/// Documented function.\nfn documented() {}\n";
        let module_b_source = "fn undocumented() {}\n";

        assert!(has_doc_comment_from_text(module_a_source, 2));
        assert!(!has_doc_comment_from_text(module_b_source, 1));

        // module_a: 1/1 = 100% coverage
        let pct_a = coverage_pct(1, 1);
        assert!((pct_a - 100.0).abs() < 0.001);
        assert!(pct_a >= 50.0); // does not need attention

        // module_b: 0/1 = 0% coverage
        let pct_b = coverage_pct(0, 1);
        assert!((pct_b - 0.0).abs() < 0.001);
        assert!(pct_b < 50.0); // needs attention
    }

    #[test]
    fn mixed_module_partial_coverage() {
        // A module with 2 documented and 3 undocumented symbols = 40% coverage
        let pct = coverage_pct(2, 5);
        assert!((pct - 40.0).abs() < 0.001);
        assert!(pct < 50.0); // needs attention
    }
}
