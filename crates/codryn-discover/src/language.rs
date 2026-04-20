use serde::{Deserialize, Serialize};
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

fn detect_by_filename(name: &str) -> Option<Language> {
    match name {
        "CMakeLists.txt" => Some(Language::CMake),
        "Dockerfile" => Some(Language::Dockerfile),
        "GNUmakefile" | "Makefile" | "makefile" => Some(Language::Makefile),
        "meson.build" | "meson.options" | "meson_options.txt" => Some(Language::Meson),
        "kustomization.yaml" | "kustomization.yml" => Some(Language::Kustomize),
        ".vimrc" => Some(Language::VimScript),
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
        _ => Language::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
