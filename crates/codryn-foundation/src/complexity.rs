/// Compute cyclomatic complexity from source text.
///
/// Counts decision points: `if`, `else if`, `elif`, `while`, `for`, `case`,
/// `catch`, `except`, `&&`, `||`, `?`.
/// Skips comment lines starting with `//`, `#`, or `/*`.
/// Returns a base complexity of 1 plus the number of decision points found.
pub fn cyclomatic_complexity(source: &str) -> u32 {
    let mut complexity: u32 = 1;
    let mut in_block_comment = false;

    for line in source.lines() {
        let trimmed = line.trim();

        // Track block comments
        if in_block_comment {
            if trimmed.contains("*/") {
                in_block_comment = false;
            }
            continue;
        }

        // Skip single-line comments
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }

        // Start of block comment
        if trimmed.starts_with("/*") {
            if !trimmed.contains("*/") {
                in_block_comment = true;
            }
            continue;
        }

        // Count decision-point keywords.
        // "else if" contains "if", so we count "else if" separately and then
        // count standalone "if" occurrences (total "if" minus "else if").
        let else_if_count = trimmed.matches("else if ").count() + trimmed.matches("elif ").count();
        let total_if_count = trimmed.matches("if ").count();
        let standalone_if = total_if_count.saturating_sub(else_if_count);

        complexity += (standalone_if + else_if_count) as u32;

        for keyword in &["while ", "for ", "case ", "catch ", "except "] {
            complexity += trimmed.matches(keyword).count() as u32;
        }

        // Count operators (no word boundary needed)
        for op in &["&&", "||", "?"] {
            complexity += trimmed.matches(op).count() as u32;
        }
    }

    complexity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_complexity_is_one() {
        let source = "fn hello() { println!(\"hi\"); }";
        assert_eq!(cyclomatic_complexity(source), 1);
    }

    #[test]
    fn counts_if_statements() {
        let source = r#"
fn example() {
    if x > 0 {
        do_something();
    }
    if y > 0 {
        do_other();
    }
}
"#;
        assert_eq!(cyclomatic_complexity(source), 3); // 1 base + 2 ifs
    }

    #[test]
    fn counts_else_if() {
        let source = r#"
fn example() {
    if x > 0 {
    } else if x < 0 {
    }
}
"#;
        // 1 base + 1 if + 1 else if
        assert_eq!(cyclomatic_complexity(source), 3);
    }

    #[test]
    fn counts_loops_and_operators() {
        let source = r#"
fn example() {
    for item in list {
        while running && active {
            if x || y {
            }
        }
    }
}
"#;
        // 1 base + 1 for + 1 while + 1 && + 1 if + 1 ||
        assert_eq!(cyclomatic_complexity(source), 6);
    }

    #[test]
    fn skips_single_line_comments() {
        let source = r#"
fn example() {
    // if this is a comment
    if real_condition {
    }
}
"#;
        assert_eq!(cyclomatic_complexity(source), 2); // 1 base + 1 if
    }

    #[test]
    fn skips_hash_comments() {
        let source = r#"
# if this is a python comment
if real_condition:
    pass
"#;
        assert_eq!(cyclomatic_complexity(source), 2); // 1 base + 1 if
    }

    #[test]
    fn skips_block_comments() {
        let source = r#"
fn example() {
    /* if this is inside
       a block comment
       while for case */
    if real {
    }
}
"#;
        assert_eq!(cyclomatic_complexity(source), 2); // 1 base + 1 if
    }

    #[test]
    fn counts_ternary_operator() {
        let source = "let x = condition ? a : b;";
        assert_eq!(cyclomatic_complexity(source), 2); // 1 base + 1 ?
    }

    #[test]
    fn counts_case_catch_except() {
        let source = r#"
switch(x) {
    case 1: break;
    case 2: break;
}
try {
} catch (e) {
}
"#;
        // 1 base + 2 case + 1 catch
        assert_eq!(cyclomatic_complexity(source), 4);
    }

    #[test]
    fn counts_python_except() {
        let source = r#"
try:
    do_something()
except ValueError:
    handle()
except TypeError:
    handle()
"#;
        // 1 base + 2 except
        assert_eq!(cyclomatic_complexity(source), 3);
    }
}
