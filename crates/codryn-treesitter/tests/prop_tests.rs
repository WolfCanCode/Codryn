use codryn_discover::Language;
use codryn_treesitter::extract_symbols;
use proptest::prelude::*;

// ── Valid Labels ──────────────────────────────────────────────────────

const VALID_LABELS: &[&str] = &[
    "Function",
    "Class",
    "Method",
    "Interface",
    "Module",
    "Impl",
    "Enum",
    "Constant",
];

// ── Strategies: Generate valid source code for each language ──────────

/// Generate a valid Java source containing a class with methods.
fn java_source_strategy() -> impl Strategy<Value = String> {
    (
        "[A-Z][a-zA-Z]{2,12}",                             // class name
        "[a-z][a-zA-Z]{2,10}",                             // method name
        prop::collection::vec("[a-z][a-zA-Z]{1,8}", 0..3), // param names
    )
        .prop_map(|(class_name, method_name, params)| {
            let param_list: String = params
                .iter()
                .map(|p| format!("int {}", p))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "public class {} {{\n    public void {}({}) {{\n    }}\n}}\n",
                class_name, method_name, param_list
            )
        })
}

/// Generate a valid Kotlin source containing a class with functions.
fn kotlin_source_strategy() -> impl Strategy<Value = String> {
    (
        "[A-Z][a-zA-Z]{2,12}",                             // class name
        "[a-z][a-zA-Z]{2,10}",                             // function name
        prop::collection::vec("[a-z][a-zA-Z]{1,8}", 0..3), // param names
    )
        .prop_map(|(class_name, func_name, params)| {
            let param_list: String = params
                .iter()
                .map(|p| format!("{}: Int", p))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "class {} {{\n    fun {}({}) {{\n    }}\n}}\n",
                class_name, func_name, param_list
            )
        })
}

/// Generate a valid Dart source containing a class with methods.
fn dart_source_strategy() -> impl Strategy<Value = String> {
    (
        "[A-Z][a-zA-Z]{2,12}",                             // class name
        "[a-z][a-zA-Z]{2,10}",                             // method name
        prop::collection::vec("[a-z][a-zA-Z]{1,8}", 0..3), // param names
    )
        .prop_map(|(class_name, method_name, params)| {
            let param_list: String = params
                .iter()
                .map(|p| format!("int {}", p))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "class {} {{\n  void {}({}) {{\n  }}\n}}\n",
                class_name, method_name, param_list
            )
        })
}

/// Generate a valid Lua source containing function definitions.
fn lua_source_strategy() -> impl Strategy<Value = String> {
    (
        "[a-z][a-zA-Z]{2,10}",                             // function name
        prop::collection::vec("[a-z][a-zA-Z]{1,8}", 0..3), // param names
    )
        .prop_map(|(func_name, params)| {
            let param_list = params.join(", ");
            format!(
                "function {}({})\n  return nil\nend\n",
                func_name, param_list
            )
        })
}

/// Generate a valid Haskell source containing function definitions.
fn haskell_source_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-zA-Z]{2,10}".prop_map(|func_name| {
        format!(
            "module Main where\n\n{} :: Int -> Int\n{} x = x + 1\n",
            func_name, func_name
        )
    })
}

// ═══════════════════════════════════════════════════════════════════════
// Property 1: Walker Output Invariants
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 1.8, 2.10, 3.6**
///
/// For any valid source containing functions/classes, all returned TsSymbol
/// elements have non-empty `name`, valid `label`, and `start_line <= end_line > 0`.
mod property1_walker_output_invariants {
    use super::*;

    /// Verify invariants on all symbols returned by a walker.
    fn assert_symbol_invariants(symbols: &[codryn_treesitter::TsSymbol], source: &str, lang: &str) {
        assert!(
            !symbols.is_empty(),
            "Expected at least one symbol from {} source:\n{}",
            lang,
            source
        );
        for sym in symbols {
            // Non-empty name
            assert!(
                !sym.name.is_empty(),
                "Symbol has empty name in {} source:\n{}",
                lang,
                source
            );
            // Valid label
            assert!(
                VALID_LABELS.contains(&sym.label.as_str()),
                "Symbol '{}' has invalid label '{}' in {} source (valid: {:?}):\n{}",
                sym.name,
                sym.label,
                lang,
                VALID_LABELS,
                source
            );
            // start_line > 0
            assert!(
                sym.start_line > 0,
                "Symbol '{}' has start_line={} (must be > 0) in {} source:\n{}",
                sym.name,
                sym.start_line,
                lang,
                source
            );
            // end_line > 0
            assert!(
                sym.end_line > 0,
                "Symbol '{}' has end_line={} (must be > 0) in {} source:\n{}",
                sym.name,
                sym.end_line,
                lang,
                source
            );
            // start_line <= end_line
            assert!(
                sym.start_line <= sym.end_line,
                "Symbol '{}' has start_line={} > end_line={} in {} source:\n{}",
                sym.name,
                sym.start_line,
                sym.end_line,
                lang,
                source
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn java_walker_output_invariants(source in java_source_strategy()) {
            let symbols = extract_symbols(Language::Java, &source)
                .expect("Java walker should return Some for valid Java source");
            assert_symbol_invariants(&symbols, &source, "Java");
        }

        #[test]
        fn kotlin_walker_output_invariants(source in kotlin_source_strategy()) {
            let symbols = extract_symbols(Language::Kotlin, &source)
                .expect("Kotlin walker should return Some for valid Kotlin source");
            assert_symbol_invariants(&symbols, &source, "Kotlin");
        }

        #[test]
        fn dart_walker_output_invariants(source in dart_source_strategy()) {
            let symbols = extract_symbols(Language::Dart, &source)
                .expect("Dart walker should return Some for valid Dart source");
            assert_symbol_invariants(&symbols, &source, "Dart");
        }

        #[test]
        fn lua_walker_output_invariants(source in lua_source_strategy()) {
            let symbols = extract_symbols(Language::Lua, &source)
                .expect("Lua walker should return Some for valid Lua source");
            assert_symbol_invariants(&symbols, &source, "Lua");
        }

        #[test]
        fn haskell_walker_output_invariants(source in haskell_source_strategy()) {
            let symbols = extract_symbols(Language::Haskell, &source)
                .expect("Haskell walker should return Some for valid Haskell source");
            assert_symbol_invariants(&symbols, &source, "Haskell");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Property 7: Tree-Sitter Extraction Produces Valid Nodes
// ═══════════════════════════════════════════════════════════════════════

/// **Validates: Requirements 10.1, 10.2, 10.3, 10.4, 10.5, 10.6, 10.7**
///
/// For any valid source file in a Tier 1 language (Java, Kotlin, Dart, Lua,
/// Haskell) containing at least one function or class definition, the
/// tree-sitter walker SHALL produce at least one node per definition, each
/// with a non-empty name, a valid label, and start_line <= end_line.
/// Functions SHALL have body_text as Some (used for cyclomatic_complexity >= 1
/// and cognitive_complexity >= 0 in the pipeline layer), while non-function
/// symbols SHALL have body_text that is either None or Some (class bodies are
/// included for fingerprinting but complexity is computed as 0 in the pipeline).
mod property7_treesitter_extraction_valid_nodes {
    use super::*;

    /// Labels that represent function-like symbols (complexity >= 1).
    const FUNCTION_LABELS: &[&str] = &["Function", "Method"];

    /// All valid labels that a Tier 1 walker may produce.
    const ALL_VALID_LABELS: &[&str] = &[
        "Function",
        "Class",
        "Method",
        "Interface",
        "Module",
        "Impl",
        "Enum",
        "Constant",
    ];

    /// Assert Property 7 invariants on all symbols returned by a Tier 1 walker.
    fn assert_property7(symbols: &[codryn_treesitter::TsSymbol], source: &str, lang: &str) {
        // At least one node must be produced
        assert!(
            !symbols.is_empty(),
            "[Property 7] Expected at least one symbol from {} source:\n{}",
            lang,
            source
        );

        for sym in symbols {
            // Non-empty name
            assert!(
                !sym.name.is_empty(),
                "[Property 7] Symbol has empty name in {} source:\n{}",
                lang,
                source
            );

            // Valid label
            assert!(
                ALL_VALID_LABELS.contains(&sym.label.as_str()),
                "[Property 7] Symbol '{}' has invalid label '{}' in {} (valid: {:?}):\n{}",
                sym.name,
                sym.label,
                lang,
                ALL_VALID_LABELS,
                source
            );

            // start_line <= end_line (both > 0 since tree-sitter is 0-indexed and we add 1)
            assert!(
                sym.start_line > 0,
                "[Property 7] Symbol '{}' has start_line={} (must be > 0) in {}:\n{}",
                sym.name,
                sym.start_line,
                lang,
                source
            );
            assert!(
                sym.end_line > 0,
                "[Property 7] Symbol '{}' has end_line={} (must be > 0) in {}:\n{}",
                sym.name,
                sym.end_line,
                lang,
                source
            );
            assert!(
                sym.start_line <= sym.end_line,
                "[Property 7] Symbol '{}' has start_line={} > end_line={} in {}:\n{}",
                sym.name,
                sym.start_line,
                sym.end_line,
                lang,
                source
            );

            // Functions/Methods SHALL have body_text as Some (for complexity computation)
            if FUNCTION_LABELS.contains(&sym.label.as_str()) {
                assert!(
                    sym.body_text.is_some(),
                    "[Property 7] Function/Method '{}' must have body_text=Some for complexity computation in {}:\n{}",
                    sym.name,
                    lang,
                    source
                );
            }
        }
    }

    // ── Java strategies with multiple definitions ──────────────────────

    /// Generate Java source with a class containing one or more methods.
    fn java_class_with_methods_strategy() -> impl Strategy<Value = String> {
        (
            "[A-Z][a-zA-Z]{2,10}",                             // class name
            prop::collection::vec("[a-z][a-zA-Z]{2,8}", 1..4), // method names
            prop::collection::vec("[a-z][a-zA-Z]{1,6}", 0..3), // param names per method
        )
            .prop_map(|(class_name, method_names, params)| {
                let mut src = format!("public class {} {{\n", class_name);
                for method_name in &method_names {
                    let param_list: String = params
                        .iter()
                        .map(|p| format!("int {}", p))
                        .collect::<Vec<_>>()
                        .join(", ");
                    src.push_str(&format!(
                        "    public void {}({}) {{\n        int x = 1;\n    }}\n",
                        method_name, param_list
                    ));
                }
                src.push_str("}\n");
                src
            })
    }

    /// Generate Java source with a standalone function (static method in a class).
    fn java_function_strategy() -> impl Strategy<Value = String> {
        (
            "[A-Z][a-zA-Z]{2,10}",
            "[a-z][a-zA-Z]{2,8}",
            prop::collection::vec("[a-z][a-zA-Z]{1,6}", 0..3),
        )
            .prop_map(|(class_name, func_name, params)| {
                let param_list: String = params
                    .iter()
                    .map(|p| format!("int {}", p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "public class {} {{\n    public static int {}({}) {{\n        return 42;\n    }}\n}}\n",
                    class_name, func_name, param_list
                )
            })
    }

    // ── Kotlin strategies ──────────────────────────────────────────────

    /// Generate Kotlin source with a class and functions.
    fn kotlin_class_with_functions_strategy() -> impl Strategy<Value = String> {
        (
            "[A-Z][a-zA-Z]{2,10}",
            prop::collection::vec("[a-z][a-zA-Z]{2,8}", 1..4),
            prop::collection::vec("[a-z][a-zA-Z]{1,6}", 0..3),
        )
            .prop_map(|(class_name, func_names, params)| {
                let mut src = format!("class {} {{\n", class_name);
                for func_name in &func_names {
                    let param_list: String = params
                        .iter()
                        .map(|p| format!("{}: Int", p))
                        .collect::<Vec<_>>()
                        .join(", ");
                    src.push_str(&format!(
                        "    fun {}({}): Int {{\n        return 1\n    }}\n",
                        func_name, param_list
                    ));
                }
                src.push_str("}\n");
                src
            })
    }

    /// Generate Kotlin top-level function.
    fn kotlin_toplevel_function_strategy() -> impl Strategy<Value = String> {
        (
            "[a-z][a-zA-Z]{2,8}",
            prop::collection::vec("[a-z][a-zA-Z]{1,6}", 0..3),
        )
            .prop_map(|(func_name, params)| {
                let param_list: String = params
                    .iter()
                    .map(|p| format!("{}: Int", p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "fun {}({}): Int {{\n    return 1\n}}\n",
                    func_name, param_list
                )
            })
    }

    // ── Dart strategies ────────────────────────────────────────────────

    /// Generate Dart source with a class and methods.
    fn dart_class_with_methods_strategy() -> impl Strategy<Value = String> {
        (
            "[A-Z][a-zA-Z]{2,10}",
            prop::collection::vec("[a-z][a-zA-Z]{2,8}", 1..4),
            prop::collection::vec("[a-z][a-zA-Z]{1,6}", 0..3),
        )
            .prop_map(|(class_name, method_names, params)| {
                let mut src = format!("class {} {{\n", class_name);
                for method_name in &method_names {
                    let param_list: String = params
                        .iter()
                        .map(|p| format!("int {}", p))
                        .collect::<Vec<_>>()
                        .join(", ");
                    src.push_str(&format!(
                        "  void {}({}) {{\n    var x = 1;\n  }}\n",
                        method_name, param_list
                    ));
                }
                src.push_str("}\n");
                src
            })
    }

    /// Generate Dart top-level function.
    fn dart_toplevel_function_strategy() -> impl Strategy<Value = String> {
        (
            "[a-z][a-zA-Z]{2,8}",
            prop::collection::vec("[a-z][a-zA-Z]{1,6}", 0..3),
        )
            .prop_map(|(func_name, params)| {
                let param_list: String = params
                    .iter()
                    .map(|p| format!("int {}", p))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("int {}({}) {{\n  return 1;\n}}\n", func_name, param_list)
            })
    }

    // ── Lua strategies ─────────────────────────────────────────────────

    /// Generate Lua source with multiple function definitions.
    fn lua_functions_strategy() -> impl Strategy<Value = String> {
        (
            prop::collection::vec("[a-z][a-zA-Z]{2,8}", 1..4),
            prop::collection::vec("[a-z][a-zA-Z]{1,6}", 0..3),
        )
            .prop_map(|(func_names, params)| {
                let mut src = String::new();
                for func_name in &func_names {
                    let param_list = params.join(", ");
                    src.push_str(&format!(
                        "function {}({})\n  local x = 1\n  return x\nend\n\n",
                        func_name, param_list
                    ));
                }
                src
            })
    }

    // ── Haskell strategies ─────────────────────────────────────────────

    /// Generate Haskell source with a module and function definitions.
    fn haskell_functions_strategy() -> impl Strategy<Value = String> {
        prop::collection::vec("[a-z][a-zA-Z]{2,8}", 1..4).prop_map(|func_names| {
            let mut src = String::from("module Main where\n\n");
            for func_name in &func_names {
                src.push_str(&format!(
                    "{} :: Int -> Int\n{} x = x + 1\n\n",
                    func_name, func_name
                ));
            }
            src
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(80))]

        // ── Java ──────────────────────────────────────────────────────

        #[test]
        fn java_class_with_methods_produces_valid_nodes(source in java_class_with_methods_strategy()) {
            let symbols = extract_symbols(Language::Java, &source)
                .expect("Java walker should return Some for valid Java source");
            assert_property7(&symbols, &source, "Java");
        }

        #[test]
        fn java_function_produces_valid_nodes(source in java_function_strategy()) {
            let symbols = extract_symbols(Language::Java, &source)
                .expect("Java walker should return Some for valid Java source");
            assert_property7(&symbols, &source, "Java");
        }

        // ── Kotlin ────────────────────────────────────────────────────

        #[test]
        fn kotlin_class_with_functions_produces_valid_nodes(source in kotlin_class_with_functions_strategy()) {
            let symbols = extract_symbols(Language::Kotlin, &source)
                .expect("Kotlin walker should return Some for valid Kotlin source");
            assert_property7(&symbols, &source, "Kotlin");
        }

        #[test]
        fn kotlin_toplevel_function_produces_valid_nodes(source in kotlin_toplevel_function_strategy()) {
            let symbols = extract_symbols(Language::Kotlin, &source)
                .expect("Kotlin walker should return Some for valid Kotlin source");
            assert_property7(&symbols, &source, "Kotlin");
        }

        // ── Dart ──────────────────────────────────────────────────────

        #[test]
        fn dart_class_with_methods_produces_valid_nodes(source in dart_class_with_methods_strategy()) {
            let symbols = extract_symbols(Language::Dart, &source)
                .expect("Dart walker should return Some for valid Dart source");
            assert_property7(&symbols, &source, "Dart");
        }

        #[test]
        fn dart_toplevel_function_produces_valid_nodes(source in dart_toplevel_function_strategy()) {
            let symbols = extract_symbols(Language::Dart, &source)
                .expect("Dart walker should return Some for valid Dart source");
            assert_property7(&symbols, &source, "Dart");
        }

        // ── Lua ───────────────────────────────────────────────────────

        #[test]
        fn lua_functions_produce_valid_nodes(source in lua_functions_strategy()) {
            let symbols = extract_symbols(Language::Lua, &source)
                .expect("Lua walker should return Some for valid Lua source");
            assert_property7(&symbols, &source, "Lua");
        }

        // ── Haskell ───────────────────────────────────────────────────

        #[test]
        fn haskell_functions_produce_valid_nodes(source in haskell_functions_strategy()) {
            let symbols = extract_symbols(Language::Haskell, &source)
                .expect("Haskell walker should return Some for valid Haskell source");
            assert_property7(&symbols, &source, "Haskell");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tier 2 Walker Unit Tests
// ═══════════════════════════════════════════════════════════════════════

mod tier2_walkers {
    use codryn_discover::Language;
    use codryn_treesitter::extract_symbols;

    #[test]
    fn julia_walker_basic() {
        let src = "function foo(x)\n    x + 1\nend\n";
        let symbols = extract_symbols(Language::Julia, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "foo" && s.label == "Function"));
    }

    #[test]
    fn julia_walker_struct_module() {
        let src = "module MyMod\nstruct Point\n    x::Float64\nend\nabstract type Shape end\nend\n";
        let symbols = extract_symbols(Language::Julia, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyMod" && s.label == "Module"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Point" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Shape" && s.label == "Interface"));
    }

    #[test]
    fn zig_walker_basic() {
        let src = "pub fn main() void {\n}\n";
        let symbols = extract_symbols(Language::Zig, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "main" && s.label == "Function" && s.is_exported));
    }

    #[test]
    fn zig_walker_struct_test() {
        let src = "const Point = struct {\n    x: f32,\n};\ntest \"point test\" {\n}\n";
        let symbols = extract_symbols(Language::Zig, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "Point" && s.label == "Class"));
        assert!(symbols.iter().any(|s| s.name == "point test" && s.is_test));
    }

    #[test]
    fn nim_walker_basic() {
        let src = "proc hello*(name: string): string =\n  \"Hello \" & name\n";
        let symbols = extract_symbols(Language::Nim, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "hello" && s.label == "Function"));
    }

    #[test]
    fn ocaml_walker_basic() {
        let src =
            "let add x y = x + y\nmodule Utils = struct end\ntype point = { x: int; y: int }\n";
        let symbols = extract_symbols(Language::OCaml, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "add" && s.label == "Function"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Utils" && s.label == "Module"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "point" && s.label == "Class"));
    }

    #[test]
    fn perl_walker_basic() {
        let src = "package MyApp::Utils;\nsub process {\n    my ($self) = @_;\n}\n";
        let symbols = extract_symbols(Language::Perl, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp::Utils" && s.label == "Module"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "process" && s.label == "Function"));
    }

    #[test]
    fn r_walker_basic() {
        let src = "add <- function(x, y) {\n  x + y\n}\nsetClass(\"Person\")\n";
        let symbols = extract_symbols(Language::R, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "add" && s.label == "Function"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Person" && s.label == "Class"));
    }

    #[test]
    fn clojure_walker_basic() {
        let src = "(ns my.app)\n(defn greet [name]\n  (str \"Hello \" name))\n(defrecord User [name age])\n(defprotocol Greetable)\n";
        let symbols = extract_symbols(Language::Clojure, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "my.app" && s.label == "Module"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "greet" && s.label == "Function"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Greetable" && s.label == "Interface"));
    }

    #[test]
    fn erlang_walker_basic() {
        let src =
            "-module(myapp).\n-export([start/0]).\n-record(state, {count}).\nstart() ->\n    ok.\n";
        let symbols = extract_symbols(Language::Erlang, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "myapp" && s.label == "Module"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "state" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "start" && s.label == "Function" && s.is_exported));
    }

    #[test]
    fn fsharp_walker_basic() {
        let src = "module MyApp\ntype Person = { Name: string }\nlet greet name = printfn \"Hello %s\" name\n";
        let symbols = extract_symbols(Language::FSharp, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyApp" && s.label == "Module"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Person" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "greet" && s.label == "Function"));
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Tier 3 Walker Unit Tests
// ═══════════════════════════════════════════════════════════════════════

mod tier3_walkers {
    use codryn_discover::Language;
    use codryn_treesitter::extract_symbols;

    #[test]
    fn hcl_walker_basic() {
        let src = "resource \"aws_instance\" \"web\" {\n  ami = \"abc\"\n}\nvariable \"region\" {\n}\nmodule \"vpc\" {\n}\noutput \"ip\" {\n}\n";
        let symbols = extract_symbols(Language::Hcl, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "aws_instance.web" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "region" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "vpc" && s.label == "Module"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ip" && s.label == "Function"));
    }

    #[test]
    fn protobuf_walker_basic() {
        let src = "message User {\n  string name = 1;\n}\nservice UserService {\n  rpc GetUser (GetUserRequest) returns (User);\n}\nenum Status {\n  ACTIVE = 0;\n}\n";
        let symbols = extract_symbols(Language::Protobuf, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "UserService" && s.label == "Interface"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "GetUser" && s.label == "Function"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.label == "Class"));
    }

    #[test]
    fn graphql_walker_basic() {
        let src = "type User {\n  id: ID!\n  name: String!\n}\ninterface Node {\n  id: ID!\n}\ninput CreateUserInput {\n  name: String!\n}\nenum Role {\n  ADMIN\n}\n";
        let symbols = extract_symbols(Language::GraphQL, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "User" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Node" && s.label == "Interface"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "CreateUserInput" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Role" && s.label == "Class"));
    }

    #[test]
    fn sql_walker_basic() {
        let src = "CREATE TABLE users (\n  id INT PRIMARY KEY\n);\nCREATE OR REPLACE FUNCTION get_user(id INT) RETURNS void;\nCREATE VIEW active_users AS SELECT * FROM users;\nCREATE PROCEDURE update_user(id INT);\n";
        let symbols = extract_symbols(Language::Sql, src).unwrap();
        assert!(symbols
            .iter()
            .any(|s| s.name == "users" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "get_user" && s.label == "Function"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "active_users" && s.label == "Class"));
        assert!(symbols
            .iter()
            .any(|s| s.name == "update_user" && s.label == "Function"));
    }
}
