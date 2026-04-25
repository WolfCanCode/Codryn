//! Tree-sitter grammar integration for Codryn.
//!
//! This crate wraps tree-sitter grammar initialization and provides a unified
//! AST walking interface for extracting symbols from source files.
//! It isolates the tree-sitter C FFI dependencies from the rest of the workspace.

mod bash_walker;
mod c_walker;
mod csharp_walker;
mod elixir_walker;
mod php_walker;
mod python_walker;
mod ruby_walker;
mod rust_walker;
mod scala_walker;
mod swift_walker;
mod ts_walker;

use codryn_discover::Language;
use tree_sitter::Parser;

/// A parameter extracted from a function/method signature.
#[derive(Debug, Clone)]
pub struct TsParam {
    pub name: String,
    pub type_name: Option<String>,
}

/// A symbol extracted from a tree-sitter AST walk.
#[derive(Debug, Clone)]
pub struct TsSymbol {
    pub name: String,
    /// Node label: "Function", "Class", "Method", "Interface", etc.
    pub label: String,
    pub start_line: i32,
    pub end_line: i32,
    /// For methods: the containing class/struct name.
    pub parent_name: Option<String>,
    pub signature: Option<String>,
    pub return_type: Option<String>,
    pub parameters: Vec<TsParam>,
    pub docstring: Option<String>,
    pub decorators: Vec<String>,
    pub base_classes: Vec<String>,
    pub is_exported: bool,
    pub is_abstract: bool,
    pub is_async: bool,
    /// Whether this symbol is a test function/class.
    pub is_test: bool,
    /// Whether this symbol is an entry point (main function, etc.).
    pub is_entry_point: bool,
    /// Raw body text for complexity computation and MinHash fingerprinting.
    pub body_text: Option<String>,
}

/// Returns a configured tree-sitter [`Parser`] for the given language, or `None`
/// if no grammar is available (e.g. Java, Kotlin, Go already have dedicated
/// AST extractors in `codryn-pipeline`).
pub fn parser_for_language(lang: Language) -> Option<Parser> {
    use tree_sitter_language::LanguageFn;

    let ts_lang: LanguageFn = match lang {
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX,
        Language::JavaScript => tree_sitter_javascript::LANGUAGE,
        Language::Python => tree_sitter_python::LANGUAGE,
        Language::Rust => tree_sitter_rust::LANGUAGE,
        Language::C => tree_sitter_c::LANGUAGE,
        Language::Cpp => tree_sitter_cpp::LANGUAGE,
        Language::CSharp => tree_sitter_c_sharp::LANGUAGE,
        Language::Ruby => tree_sitter_ruby::LANGUAGE,
        Language::Php => tree_sitter_php::LANGUAGE_PHP,
        Language::Swift => tree_sitter_swift::LANGUAGE,
        Language::Scala => tree_sitter_scala::LANGUAGE,
        Language::Elixir => tree_sitter_elixir::LANGUAGE,
        Language::Bash => tree_sitter_bash::LANGUAGE,
        // Java, Kotlin, Go already have AST extractors in codryn-pipeline
        Language::Java | Language::Kotlin | Language::Go => return None,
        _ => return None,
    };

    let mut parser = Parser::new();
    parser.set_language(&ts_lang.into()).ok()?;
    Some(parser)
}

/// Walk a tree-sitter AST and extract symbols for the given language.
///
/// Returns `None` if:
/// - No tree-sitter grammar is available for the language (caller should fall
///   back to regex).
/// - The parser fails to produce any tree at all.
///
/// Note: partial parse errors (common with newer syntax, JSX, etc.) are
/// tolerated — tree-sitter is designed for error recovery and the rest of the
/// tree is still usable.
pub fn extract_symbols(lang: Language, source: &str) -> Option<Vec<TsSymbol>> {
    let mut parser = parser_for_language(lang)?;
    let tree = parser.parse(source, None)?;

    if tree.root_node().has_error() {
        tracing::debug!(
            "tree-sitter: partial parse errors detected, proceeding with recovered tree"
        );
    }

    let symbols = match lang {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            ts_walker::walk_tree(&tree, source)
        }
        Language::Python => python_walker::walk_tree(&tree, source),
        Language::Rust => rust_walker::walk_tree(&tree, source),
        Language::C | Language::Cpp => c_walker::walk_tree(&tree, source),
        Language::CSharp => csharp_walker::walk_tree(&tree, source),
        Language::Ruby => ruby_walker::walk_tree(&tree, source),
        Language::Php => php_walker::walk_tree(&tree, source),
        Language::Swift => swift_walker::walk_tree(&tree, source),
        Language::Scala => scala_walker::walk_tree(&tree, source),
        Language::Elixir => elixir_walker::walk_tree(&tree, source),
        Language::Bash => bash_walker::walk_tree(&tree, source),
        _ => return None,
    };

    Some(symbols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_discover::Language;

    // -----------------------------------------------------------------------
    // 1. TypeScript extraction
    // -----------------------------------------------------------------------

    #[test]
    fn ts_function_declaration() {
        let src = r#"function greet(name: string): string { return "hi " + name; }"#;
        let syms = extract_symbols(Language::TypeScript, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "greet")
            .expect("greet not found");
        assert_eq!(f.label, "Function");
        assert!(!f.is_async);
        assert_eq!(f.parameters.len(), 1);
        assert_eq!(f.parameters[0].name, "name");
        assert_eq!(f.parameters[0].type_name.as_deref(), Some("string"));
        assert_eq!(f.return_type.as_deref(), Some("string"));
    }

    #[test]
    fn ts_class_with_methods() {
        let src = r#"
class Animal {
    constructor(name: string) {}
    speak(): void {}
}
"#;
        let syms = extract_symbols(Language::TypeScript, src).unwrap();
        let cls = syms
            .iter()
            .find(|s| s.name == "Animal" && s.label == "Class");
        assert!(cls.is_some(), "Class Animal not found");
        let methods: Vec<_> = syms
            .iter()
            .filter(|s| s.label == "Method" && s.parent_name.as_deref() == Some("Animal"))
            .collect();
        assert!(
            methods.len() >= 2,
            "Expected at least constructor + speak, got {}",
            methods.len()
        );
    }

    #[test]
    fn ts_nested_class() {
        let src = r#"
class Outer {
    method() {
        class Inner {
            innerMethod() {}
        }
    }
}
"#;
        let syms = extract_symbols(Language::TypeScript, src).unwrap();
        let outer = syms
            .iter()
            .find(|s| s.name == "Outer" && s.label == "Class");
        assert!(outer.is_some(), "Outer class not found");
        let inner = syms
            .iter()
            .find(|s| s.name == "Inner" && s.label == "Class");
        assert!(inner.is_some(), "Nested Inner class not found");
    }

    #[test]
    fn ts_arrow_function() {
        let src = r#"const add = (a: number, b: number): number => a + b;"#;
        let syms = extract_symbols(Language::TypeScript, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "add")
            .expect("add not found");
        assert_eq!(f.label, "Function");
        assert_eq!(f.parameters.len(), 2);
    }

    #[test]
    fn ts_interface() {
        let src = r#"
interface Printable {
    print(): void;
}
"#;
        let syms = extract_symbols(Language::TypeScript, src).unwrap();
        let iface = syms
            .iter()
            .find(|s| s.name == "Printable" && s.label == "Interface");
        assert!(iface.is_some(), "Interface Printable not found");
    }

    #[test]
    fn ts_async_function() {
        let src = r#"async function fetchData(url: string): Promise<string> { return ""; }"#;
        let syms = extract_symbols(Language::TypeScript, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "fetchData")
            .expect("fetchData not found");
        assert!(f.is_async);
        assert_eq!(f.return_type.as_deref(), Some("Promise<string>"));
    }

    // -----------------------------------------------------------------------
    // 2. Python extraction
    // -----------------------------------------------------------------------

    #[test]
    fn py_function() {
        let src = r#"
def greet(name) -> str:
    """Say hello."""
    return "hi " + name
"#;
        let syms = extract_symbols(Language::Python, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "greet")
            .expect("greet not found");
        assert_eq!(f.label, "Function");
        assert_eq!(f.parameters.len(), 1);
        assert_eq!(f.parameters[0].name, "name");
        assert_eq!(f.return_type.as_deref(), Some("str"));
        assert!(f.docstring.as_ref().unwrap().contains("Say hello"));
    }

    #[test]
    fn py_class_with_methods() {
        let src = r#"
class Dog:
    """A dog class."""
    def __init__(self, name: str):
        self.name = name

    def bark(self) -> str:
        return "woof"
"#;
        let syms = extract_symbols(Language::Python, src).unwrap();
        let cls = syms.iter().find(|s| s.name == "Dog" && s.label == "Class");
        assert!(cls.is_some(), "Class Dog not found");
        assert!(cls
            .unwrap()
            .docstring
            .as_ref()
            .unwrap()
            .contains("A dog class"));
        let methods: Vec<_> = syms
            .iter()
            .filter(|s| s.label == "Method" && s.parent_name.as_deref() == Some("Dog"))
            .collect();
        assert!(
            methods.len() >= 2,
            "Expected __init__ + bark, got {}",
            methods.len()
        );
    }

    #[test]
    fn py_nested_class() {
        let src = r#"
class Outer:
    class Inner:
        def inner_method(self):
            pass
"#;
        let syms = extract_symbols(Language::Python, src).unwrap();
        let inner = syms
            .iter()
            .find(|s| s.name == "Inner" && s.label == "Class");
        assert!(inner.is_some(), "Nested Inner class not found");
        assert_eq!(inner.unwrap().parent_name.as_deref(), Some("Outer"));
    }

    #[test]
    fn py_decorator() {
        let src = r#"
@staticmethod
def helper():
    pass
"#;
        let syms = extract_symbols(Language::Python, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "helper")
            .expect("helper not found");
        assert!(!f.decorators.is_empty(), "Expected decorators on helper");
        assert!(f.decorators.iter().any(|d| d.contains("staticmethod")));
    }

    #[test]
    fn py_docstring_extraction() {
        let src = r#"
def compute(x: int) -> int:
    """
    Compute the result.

    Args:
        x: input value
    """
    return x * 2
"#;
        let syms = extract_symbols(Language::Python, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "compute")
            .expect("compute not found");
        let doc = f.docstring.as_ref().expect("docstring missing");
        assert!(doc.contains("Compute the result"));
    }

    // -----------------------------------------------------------------------
    // 3. Rust extraction
    // -----------------------------------------------------------------------

    #[test]
    fn rust_function() {
        let src = r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#;
        let syms = extract_symbols(Language::Rust, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "add" && s.label == "Function")
            .expect("add not found");
        assert!(f.is_exported);
        assert_eq!(f.parameters.len(), 2);
        assert_eq!(f.return_type.as_deref(), Some("i32"));
    }

    #[test]
    fn rust_struct_and_impl() {
        let src = r#"
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    pub fn distance(&self, other: &Point) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}
"#;
        let syms = extract_symbols(Language::Rust, src).unwrap();
        let st = syms
            .iter()
            .find(|s| s.name == "Point" && s.label == "Class");
        assert!(st.is_some(), "Struct Point not found");
        assert!(st.unwrap().is_exported);

        let impl_sym = syms.iter().find(|s| s.name == "Point" && s.label == "Impl");
        assert!(impl_sym.is_some(), "Impl Point not found");

        let methods: Vec<_> = syms
            .iter()
            .filter(|s| s.label == "Method" && s.parent_name.as_deref() == Some("Point"))
            .collect();
        assert!(
            methods.len() >= 2,
            "Expected new + distance methods, got {}",
            methods.len()
        );
    }

    #[test]
    fn rust_trait() {
        let src = r#"
pub trait Drawable {
    fn draw(&self);
    fn area(&self) -> f64;
}
"#;
        let syms = extract_symbols(Language::Rust, src).unwrap();
        let tr = syms
            .iter()
            .find(|s| s.name == "Drawable" && s.label == "Interface");
        assert!(tr.is_some(), "Trait Drawable not found");
        assert!(tr.unwrap().is_exported);
        assert!(tr.unwrap().is_abstract);

        let methods: Vec<_> = syms
            .iter()
            .filter(|s| s.label == "Method" && s.parent_name.as_deref() == Some("Drawable"))
            .collect();
        assert!(
            methods.len() >= 2,
            "Expected draw + area trait methods, got {}",
            methods.len()
        );
    }

    #[test]
    fn rust_async_fn() {
        let src = r#"
pub async fn fetch(url: &str) -> Result<String, Error> {
    Ok(String::new())
}
"#;
        let syms = extract_symbols(Language::Rust, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "fetch")
            .expect("fetch not found");
        assert!(f.is_exported);
        assert_eq!(f.label, "Function");
        assert!(!f.parameters.is_empty());
        let sig = f.signature.as_ref().unwrap();
        assert!(
            sig.contains("fn fetch"),
            "Signature should contain fn fetch: {}",
            sig
        );
    }

    #[test]
    fn rust_pub_visibility() {
        let src = r#"
fn private_fn() {}
pub fn public_fn() {}
"#;
        let syms = extract_symbols(Language::Rust, src).unwrap();
        let priv_fn = syms.iter().find(|s| s.name == "private_fn").unwrap();
        assert!(!priv_fn.is_exported);
        let pub_fn = syms.iter().find(|s| s.name == "public_fn").unwrap();
        assert!(pub_fn.is_exported);
    }

    // -----------------------------------------------------------------------
    // 4. C extraction
    // -----------------------------------------------------------------------

    #[test]
    fn c_function_definition() {
        let src = r#"
int add(int a, int b) {
    return a + b;
}
"#;
        let syms = extract_symbols(Language::C, src).unwrap();
        let f = syms
            .iter()
            .find(|s| s.name == "add" && s.label == "Function")
            .expect("add not found");
        assert_eq!(f.return_type.as_deref(), Some("int"));
        assert_eq!(f.parameters.len(), 2);
    }

    #[test]
    fn c_struct() {
        let src = r#"
struct Point {
    int x;
    int y;
};
"#;
        let syms = extract_symbols(Language::C, src).unwrap();
        let st = syms
            .iter()
            .find(|s| s.name == "Point" && s.label == "Class");
        assert!(st.is_some(), "Struct Point not found");
    }

    // -----------------------------------------------------------------------
    // 5. Fallback: unsupported language returns None
    // -----------------------------------------------------------------------

    #[test]
    fn unsupported_language_returns_none() {
        let src = "public class Main { public static void main(String[] args) {} }";
        let result = extract_symbols(Language::Java, src);
        assert!(
            result.is_none(),
            "Java should return None (no tree-sitter grammar)"
        );
    }

    #[test]
    fn unknown_language_returns_none() {
        let src = "some random content";
        let result = extract_symbols(Language::Unknown, src);
        assert!(result.is_none(), "Unknown language should return None");
    }

    // -----------------------------------------------------------------------
    // 6. Parse error: malformed source still returns Some (tree-sitter recovers)
    // -----------------------------------------------------------------------

    #[test]
    fn malformed_typescript_still_parses() {
        // Severely malformed source — tree-sitter will flag errors but still
        // produce a (partial) tree.  We no longer bail out on has_error().
        let src = r#"function {{{{{ class >>><<< !!!!"#;
        let result = extract_symbols(Language::TypeScript, src);
        assert!(
            result.is_some(),
            "Malformed TypeScript should still return Some (tree-sitter error recovery)"
        );
    }
}
