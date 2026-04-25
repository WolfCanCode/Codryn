use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Bash,
    C,
    Cpp,
    CSharp,
    Clojure,
    CMake,
    Cobol,
    CommonLisp,
    Css,
    Cuda,
    Dart,
    Dockerfile,
    Elixir,
    Elm,
    EmacsLisp,
    Erlang,
    FSharp,
    Form,
    Fortran,
    Glsl,
    Go,
    GraphQL,
    Groovy,
    Haskell,
    Hcl,
    Html,
    Ini,
    Java,
    JavaScript,
    Json,
    Julia,
    Kotlin,
    Kustomize,
    Lean,
    Lua,
    Magma,
    Makefile,
    Markdown,
    Matlab,
    Meson,
    Nix,
    OCaml,
    Perl,
    Php,
    Protobuf,
    Python,
    R,
    Ruby,
    Rust,
    Scala,
    Scss,
    Sql,
    Svelte,
    Swift,
    Toml,
    Tsx,
    TypeScript,
    Verilog,
    VimScript,
    Vue,
    Wolfram,
    Xml,
    Yaml,
    Zig,
    // Phase 2 additions
    Ada,
    Astro,
    Awk,
    Crystal,
    DLang,
    Fennel,
    Fish,
    GDScript,
    Gleam,
    Hare,
    Janet,
    Json5,
    Jsonnet,
    Just,
    Kdl,
    Liquid,
    Luau,
    Move,
    Nim,
    ObjectiveC,
    Odin,
    Pascal,
    Starlark,
    Unknown,
}

impl Language {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bash => "Bash",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Clojure => "Clojure",
            Self::CMake => "CMake",
            Self::Cobol => "COBOL",
            Self::CommonLisp => "Common Lisp",
            Self::Css => "CSS",
            Self::Cuda => "CUDA",
            Self::Dart => "Dart",
            Self::Dockerfile => "Dockerfile",
            Self::Elixir => "Elixir",
            Self::Elm => "Elm",
            Self::EmacsLisp => "Emacs Lisp",
            Self::Erlang => "Erlang",
            Self::FSharp => "F#",
            Self::Form => "FORM",
            Self::Fortran => "Fortran",
            Self::Glsl => "GLSL",
            Self::Go => "Go",
            Self::GraphQL => "GraphQL",
            Self::Groovy => "Groovy",
            Self::Haskell => "Haskell",
            Self::Hcl => "HCL",
            Self::Html => "HTML",
            Self::Ini => "INI",
            Self::Java => "Java",
            Self::JavaScript => "JavaScript",
            Self::Json => "JSON",
            Self::Julia => "Julia",
            Self::Kotlin => "Kotlin",
            Self::Kustomize => "Kustomize",
            Self::Lean => "Lean",
            Self::Lua => "Lua",
            Self::Magma => "Magma",
            Self::Makefile => "Makefile",
            Self::Markdown => "Markdown",
            Self::Matlab => "MATLAB",
            Self::Meson => "Meson",
            Self::Nix => "Nix",
            Self::OCaml => "OCaml",
            Self::Perl => "Perl",
            Self::Php => "PHP",
            Self::Protobuf => "Protobuf",
            Self::Python => "Python",
            Self::R => "R",
            Self::Ruby => "Ruby",
            Self::Rust => "Rust",
            Self::Scala => "Scala",
            Self::Scss => "SCSS",
            Self::Sql => "SQL",
            Self::Svelte => "Svelte",
            Self::Swift => "Swift",
            Self::Toml => "TOML",
            Self::Tsx => "TSX",
            Self::TypeScript => "TypeScript",
            Self::Verilog => "Verilog",
            Self::VimScript => "VimScript",
            Self::Vue => "Vue",
            Self::Wolfram => "Wolfram",
            Self::Xml => "XML",
            Self::Yaml => "YAML",
            Self::Zig => "Zig",
            Self::Ada => "Ada",
            Self::Astro => "Astro",
            Self::Awk => "AWK",
            Self::Crystal => "Crystal",
            Self::DLang => "D",
            Self::Fennel => "Fennel",
            Self::Fish => "Fish",
            Self::GDScript => "GDScript",
            Self::Gleam => "Gleam",
            Self::Hare => "Hare",
            Self::Janet => "Janet",
            Self::Json5 => "JSON5",
            Self::Jsonnet => "Jsonnet",
            Self::Just => "Just",
            Self::Kdl => "KDL",
            Self::Liquid => "Liquid",
            Self::Luau => "Luau",
            Self::Move => "Move",
            Self::Nim => "Nim",
            Self::ObjectiveC => "Objective-C",
            Self::Odin => "Odin",
            Self::Pascal => "Pascal",
            Self::Starlark => "Starlark",
            Self::Unknown => "Unknown",
        }
    }
}

/// Detect language from a relative file path.
pub fn detect(rel_path: &str) -> Language {
    let filename = Path::new(rel_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");

    // Check special filenames first
    if let Some(lang) = detect_by_filename(filename) {
        return lang;
    }

    // Then by extension
    if let Some(ext) = Path::new(rel_path).extension().and_then(|e| e.to_str()) {
        detect_by_extension(ext)
    } else {
        Language::Unknown
    }
}

/// Detect language from a relative file path, checking user mappings first.
/// Falls back to built-in `detect()` if no user mapping matches.
pub fn detect_with_mappings(rel_path: &str, mappings: &HashMap<String, Language>) -> Language {
    if let Some(ext) = Path::new(rel_path).extension().and_then(|e| e.to_str()) {
        if let Some(&lang) = mappings.get(ext) {
            return lang;
        }
    }
    detect(rel_path)
}

fn detect_by_filename(name: &str) -> Option<Language> {
    match name {
        "CMakeLists.txt" => Some(Language::CMake),
        "Dockerfile" => Some(Language::Dockerfile),
        "GNUmakefile" | "Makefile" | "makefile" => Some(Language::Makefile),
        "meson.build" | "meson.options" | "meson_options.txt" => Some(Language::Meson),
        "kustomization.yaml" | "kustomization.yml" => Some(Language::Kustomize),
        ".vimrc" => Some(Language::VimScript),
        // Phase 2 filename mappings
        "Justfile" | "justfile" => Some(Language::Just),
        "BUILD" | "BUILD.bazel" | "WORKSPACE" | "WORKSPACE.bazel" => Some(Language::Starlark),
        _ if name.starts_with("Dockerfile") => Some(Language::Dockerfile),
        _ => None,
    }
}

fn detect_by_extension(ext: &str) -> Language {
    match ext.to_lowercase().as_str() {
        "bash" | "sh" => Language::Bash,
        "c" => Language::C,
        "cc" | "ccm" | "cpp" | "cppm" | "cxx" | "h" | "hh" | "hpp" | "hxx" | "ixx" => Language::Cpp,
        "cs" => Language::CSharp,
        "clj" | "cljc" | "cljs" => Language::Clojure,
        "cmake" => Language::CMake,
        "cbl" | "cob" => Language::Cobol,
        "cl" | "lisp" | "lsp" => Language::CommonLisp,
        "css" => Language::Css,
        "cu" | "cuh" => Language::Cuda,
        "dart" => Language::Dart,
        "dockerfile" => Language::Dockerfile,
        "ex" | "exs" => Language::Elixir,
        "elm" => Language::Elm,
        "el" => Language::EmacsLisp,
        "erl" => Language::Erlang,
        "fs" | "fsi" | "fsx" => Language::FSharp,
        "frm" | "prc" => Language::Form,
        "f03" | "f08" | "f90" | "f95" => Language::Fortran,
        "frag" | "glsl" | "vert" => Language::Glsl,
        "go" => Language::Go,
        "gql" | "graphql" => Language::GraphQL,
        "gradle" | "groovy" => Language::Groovy,
        "hs" => Language::Haskell,
        "hcl" | "tf" => Language::Hcl,
        "htm" | "html" => Language::Html,
        "cfg" | "conf" | "ini" => Language::Ini,
        "java" => Language::Java,
        "js" | "jsx" => Language::JavaScript,
        "json" => Language::Json,
        "jl" => Language::Julia,
        "kt" | "kts" => Language::Kotlin,
        "lean" => Language::Lean,
        "lua" => Language::Lua,
        "mag" | "magma" => Language::Magma,
        "mk" => Language::Makefile,
        "md" | "mdx" => Language::Markdown,
        "matlab" | "mlx" => Language::Matlab,
        "meson" => Language::Meson,
        "nix" => Language::Nix,
        "ml" | "mli" => Language::OCaml,
        "pl" | "pm" => Language::Perl,
        "php" => Language::Php,
        "proto" => Language::Protobuf,
        "py" => Language::Python,
        "r" => Language::R,
        "gemspec" | "rake" | "rb" => Language::Ruby,
        "rs" => Language::Rust,
        "sc" | "scala" => Language::Scala,
        "scss" => Language::Scss,
        "sql" => Language::Sql,
        "svelte" => Language::Svelte,
        "swift" => Language::Swift,
        "sv" | "v" => Language::Verilog,
        "toml" => Language::Toml,
        "tsx" => Language::Tsx,
        "ts" => Language::TypeScript,
        "vim" | "vimrc" => Language::VimScript,
        "vue" => Language::Vue,
        "wl" | "wls" => Language::Wolfram,
        "xml" | "xsd" | "xsl" | "svg" => Language::Xml,
        "yaml" | "yml" => Language::Yaml,
        "zig" => Language::Zig,
        // Phase 2 additions
        "adb" | "ads" => Language::Ada,
        "astro" => Language::Astro,
        "awk" => Language::Awk,
        "cr" => Language::Crystal,
        "d" => Language::DLang,
        "fnl" => Language::Fennel,
        "fish" => Language::Fish,
        "gd" => Language::GDScript,
        "gleam" => Language::Gleam,
        "ha" => Language::Hare,
        "janet" => Language::Janet,
        "json5" => Language::Json5,
        "jsonnet" | "libsonnet" => Language::Jsonnet,
        "just" => Language::Just,
        "kdl" => Language::Kdl,
        "liquid" => Language::Liquid,
        "luau" => Language::Luau,
        "move" => Language::Move,
        "nim" | "nims" | "nimble" => Language::Nim,
        "m" => Language::Matlab,
        "mm" => Language::ObjectiveC,
        "odin" => Language::Odin,
        "pas" | "pp" | "lpr" => Language::Pascal,
        "star" | "bzl" => Language::Starlark,
        _ => Language::Unknown,
    }
}

/// Disambiguate `.m` files by scanning content for language-specific markers.
/// Reads the first 4096 bytes of the file.
pub fn disambiguate_m_file(path: &Path) -> Language {
    let content = match fs::read(path) {
        Ok(bytes) => {
            let len = bytes.len().min(4096);
            String::from_utf8_lossy(&bytes[..len]).into_owned()
        }
        Err(_) => return Language::Matlab, // Default fallback
    };

    // Objective-C markers (check first — more distinctive)
    const OBJC_MARKERS: &[&str] = &[
        "#import",
        "#include",
        "@interface",
        "@implementation",
        "@protocol",
        "@property",
        "NSObject",
        "NSString",
    ];
    let objc_score: usize = OBJC_MARKERS.iter().filter(|m| content.contains(*m)).count();
    if objc_score >= 2 {
        return Language::ObjectiveC;
    }

    // Magma markers (check before MATLAB — more distinctive)
    const MAGMA_MARKERS: &[&str] = &["intrinsic", "forward", "declare verbose"];
    let magma_score: usize = MAGMA_MARKERS
        .iter()
        .filter(|m| content.contains(*m))
        .count();
    if magma_score >= 1 {
        return Language::Magma;
    }

    // MATLAB markers
    const MATLAB_MARKERS: &[&str] = &["function ", "classdef ", "end\n", "end\r"];
    let matlab_score: usize = MATLAB_MARKERS
        .iter()
        .filter(|m| content.contains(*m))
        .count();
    if matlab_score >= 1 {
        return Language::Matlab;
    }

    // Fall back to comment ratio: % (MATLAB) vs // (Objective-C)
    let pct_comments = content
        .lines()
        .filter(|l| l.trim_start().starts_with('%'))
        .count();
    let slash_comments = content
        .lines()
        .filter(|l| l.trim_start().starts_with("//"))
        .count();
    if pct_comments > slash_comments {
        return Language::Matlab;
    }
    if slash_comments > 0 {
        return Language::ObjectiveC;
    }

    // Default to MATLAB
    Language::Matlab
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_detect_extensions() {
        assert_eq!(detect("src/main.rs"), Language::Rust);
        assert_eq!(detect("lib/utils.py"), Language::Python);
        assert_eq!(detect("app.tsx"), Language::Tsx);
        assert_eq!(detect("go.mod"), Language::Unknown);
        assert_eq!(detect("main.go"), Language::Go);
    }

    #[test]
    fn test_detect_filenames() {
        assert_eq!(detect("Dockerfile"), Language::Dockerfile);
        assert_eq!(detect("Makefile"), Language::Makefile);
        assert_eq!(detect("CMakeLists.txt"), Language::CMake);
        assert_eq!(detect("kustomization.yaml"), Language::Kustomize);
    }

    // --- Phase 2: New extension mappings (Req 8.2) ---

    #[test]
    fn test_detect_phase2_extensions() {
        let cases = vec![
            ("src/main.adb", Language::Ada),
            ("src/spec.ads", Language::Ada),
            ("page.astro", Language::Astro),
            ("script.awk", Language::Awk),
            ("app.cr", Language::Crystal),
            ("lib.d", Language::DLang),
            ("init.fnl", Language::Fennel),
            ("config.fish", Language::Fish),
            ("player.gd", Language::GDScript),
            ("app.gleam", Language::Gleam),
            ("main.ha", Language::Hare),
            ("repl.janet", Language::Janet),
            ("config.json5", Language::Json5),
            ("lib.jsonnet", Language::Jsonnet),
            ("lib.libsonnet", Language::Jsonnet),
            ("build.just", Language::Just),
            ("config.kdl", Language::Kdl),
            ("template.liquid", Language::Liquid),
            ("module.luau", Language::Luau),
            ("contract.move", Language::Move),
            ("app.nim", Language::Nim),
            ("config.nims", Language::Nim),
            ("pkg.nimble", Language::Nim),
            ("hello.mm", Language::ObjectiveC),
            ("game.odin", Language::Odin),
            ("unit.pas", Language::Pascal),
            ("unit.pp", Language::Pascal),
            ("project.lpr", Language::Pascal),
            ("rules.star", Language::Starlark),
            ("defs.bzl", Language::Starlark),
        ];
        for (path, expected) in cases {
            assert_eq!(detect(path), expected, "failed for path: {}", path);
        }
    }

    #[test]
    fn test_detect_m_extension_defaults_to_matlab() {
        // .m defaults to Matlab via detect_by_extension (before disambiguation)
        assert_eq!(detect("script.m"), Language::Matlab);
    }

    // --- Phase 2: New filename mappings (Req 8.3) ---

    #[test]
    fn test_detect_phase2_filenames() {
        assert_eq!(detect("Justfile"), Language::Just);
        assert_eq!(detect("justfile"), Language::Just);
        assert_eq!(detect("BUILD"), Language::Starlark);
        assert_eq!(detect("BUILD.bazel"), Language::Starlark);
        assert_eq!(detect("WORKSPACE"), Language::Starlark);
        assert_eq!(detect("WORKSPACE.bazel"), Language::Starlark);
    }

    // --- Phase 2: Language::name() returns non-empty for all new variants (Req 8.5) ---

    #[test]
    fn test_phase2_language_names_non_empty() {
        let phase2_variants = vec![
            Language::Ada,
            Language::Astro,
            Language::Awk,
            Language::Crystal,
            Language::DLang,
            Language::Fennel,
            Language::Fish,
            Language::GDScript,
            Language::Gleam,
            Language::Hare,
            Language::Janet,
            Language::Json5,
            Language::Jsonnet,
            Language::Just,
            Language::Kdl,
            Language::Liquid,
            Language::Luau,
            Language::Move,
            Language::Nim,
            Language::ObjectiveC,
            Language::Odin,
            Language::Pascal,
            Language::Starlark,
        ];
        for lang in phase2_variants {
            let name = lang.name();
            assert!(!name.is_empty(), "{:?} has empty name", lang);
        }
    }

    // --- Phase 2: .m disambiguation tests (Req 14.2, 14.3, 14.4, 14.5) ---

    fn write_temp_m_file(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".m")
            .tempfile()
            .expect("failed to create temp file");
        f.write_all(content.as_bytes())
            .expect("failed to write temp file");
        f.flush().expect("failed to flush");
        f
    }

    #[test]
    fn test_disambiguate_m_objective_c() {
        // Two Objective-C markers → ObjectiveC
        let f = write_temp_m_file(
            "#import <Foundation/Foundation.h>\n@interface MyClass : NSObject\n@end\n",
        );
        assert_eq!(disambiguate_m_file(f.path()), Language::ObjectiveC);
    }

    #[test]
    fn test_disambiguate_m_objective_c_nsstring() {
        let f = write_temp_m_file("#include <stdio.h>\nNSString *name = @\"hello\";\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::ObjectiveC);
    }

    #[test]
    fn test_disambiguate_m_matlab() {
        let f = write_temp_m_file("function result = add(a, b)\n  result = a + b;\nend\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::Matlab);
    }

    #[test]
    fn test_disambiguate_m_matlab_classdef() {
        let f = write_temp_m_file("classdef MyClass\n  properties\n    x\n  end\nend\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::Matlab);
    }

    #[test]
    fn test_disambiguate_m_magma() {
        let f = write_temp_m_file("intrinsic foo(x :: RngIntElt) -> RngIntElt\n{ return x; }\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::Magma);
    }

    #[test]
    fn test_disambiguate_m_magma_forward() {
        let f = write_temp_m_file("forward bar;\nsome code here\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::Magma);
    }

    #[test]
    fn test_disambiguate_m_magma_declare_verbose() {
        let f = write_temp_m_file("declare verbose MyPackage, 2;\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::Magma);
    }

    #[test]
    fn test_disambiguate_m_ambiguous_defaults_to_matlab() {
        // No markers at all → defaults to Matlab
        let f = write_temp_m_file("x = 42;\ny = x + 1;\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::Matlab);
    }

    #[test]
    fn test_disambiguate_m_comment_ratio_matlab() {
        // More % comments than // comments → Matlab
        let f = write_temp_m_file("% this is a comment\n% another comment\nx = 1;\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::Matlab);
    }

    #[test]
    fn test_disambiguate_m_comment_ratio_objc() {
        // More // comments than % comments, no other markers → ObjectiveC
        let f = write_temp_m_file("// this is a comment\n// another comment\nint x = 1;\n");
        assert_eq!(disambiguate_m_file(f.path()), Language::ObjectiveC);
    }

    #[test]
    fn test_disambiguate_m_nonexistent_file() {
        // Non-existent file → defaults to Matlab
        let result = disambiguate_m_file(Path::new("/tmp/nonexistent_file_xyz.m"));
        assert_eq!(result, Language::Matlab);
    }

    #[test]
    fn test_disambiguate_m_single_objc_marker_not_enough() {
        // Only 1 Objective-C marker is not enough (needs >= 2)
        let f = write_temp_m_file("#import <stdio.h>\nx = 42;\n");
        // Falls through to Magma check (no match), then MATLAB check (no match),
        // then comment ratio (no % or // comments) → defaults to Matlab
        assert_eq!(disambiguate_m_file(f.path()), Language::Matlab);
    }

    // --- detect_with_mappings tests ---

    #[test]
    fn test_detect_with_mappings_user_override() {
        let mut mappings = HashMap::new();
        mappings.insert("rs".to_string(), Language::Python);
        assert_eq!(
            detect_with_mappings("src/main.rs", &mappings),
            Language::Python
        );
    }

    #[test]
    fn test_detect_with_mappings_fallback_to_builtin() {
        let mappings = HashMap::new();
        assert_eq!(
            detect_with_mappings("src/main.rs", &mappings),
            Language::Rust
        );
    }

    #[test]
    fn test_detect_with_mappings_no_extension() {
        let mut mappings = HashMap::new();
        mappings.insert("rs".to_string(), Language::Python);
        // File with no extension falls back to detect() which checks filenames
        assert_eq!(
            detect_with_mappings("Makefile", &mappings),
            Language::Makefile
        );
    }

    #[test]
    fn test_detect_with_mappings_custom_extension() {
        let mut mappings = HashMap::new();
        mappings.insert("xyz".to_string(), Language::Go);
        assert_eq!(detect_with_mappings("app.xyz", &mappings), Language::Go);
    }
}
