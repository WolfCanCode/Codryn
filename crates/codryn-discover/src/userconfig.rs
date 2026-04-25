use crate::language::Language;
use std::collections::HashMap;
use std::path::Path;

/// User-defined extension-to-language mappings.
pub type LanguageMappings = HashMap<String, Language>;

/// Load language mappings from global and project-level config files.
/// Project-level mappings override global ones.
pub fn load_language_mappings(repo_root: &Path) -> LanguageMappings {
    let mut mappings = LanguageMappings::new();

    // 1. Load global config: $XDG_CONFIG_HOME/codebase-memory-mcp/config.json
    if let Some(global_path) = global_config_path() {
        load_mappings_from_file(&global_path, &mut mappings);
    }

    // 2. Load project config: {repo_root}/.codebase-memory.json (overrides global)
    let project_path = repo_root.join(".codebase-memory.json");
    load_mappings_from_file(&project_path, &mut mappings);

    mappings
}

fn global_config_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("codebase-memory-mcp").join("config.json"))
}

fn load_mappings_from_file(path: &Path, mappings: &mut LanguageMappings) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "invalid config JSON");
            return;
        }
    };
    if let Some(lm) = json.get("language_mappings").and_then(|v| v.as_object()) {
        for (ext, lang_name) in lm {
            if let Some(lang_str) = lang_name.as_str() {
                match parse_language_name(lang_str) {
                    Some(lang) => {
                        mappings.insert(ext.clone(), lang);
                    }
                    None => {
                        tracing::warn!(
                            ext = %ext,
                            lang = %lang_str,
                            "unrecognized language name in config"
                        );
                    }
                }
            }
        }
    }
}

/// Parse a language name string into a Language variant (case-insensitive).
/// Matches against the display names returned by `Language::name()`.
pub fn parse_language_name(name: &str) -> Option<Language> {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "bash" => Some(Language::Bash),
        "c" => Some(Language::C),
        "c++" | "cpp" => Some(Language::Cpp),
        "c#" | "csharp" => Some(Language::CSharp),
        "clojure" => Some(Language::Clojure),
        "cmake" => Some(Language::CMake),
        "cobol" => Some(Language::Cobol),
        "common lisp" | "commonlisp" => Some(Language::CommonLisp),
        "css" => Some(Language::Css),
        "cuda" => Some(Language::Cuda),
        "dart" => Some(Language::Dart),
        "dockerfile" => Some(Language::Dockerfile),
        "elixir" => Some(Language::Elixir),
        "elm" => Some(Language::Elm),
        "emacs lisp" | "emacslisp" => Some(Language::EmacsLisp),
        "erlang" => Some(Language::Erlang),
        "f#" | "fsharp" => Some(Language::FSharp),
        "form" => Some(Language::Form),
        "fortran" => Some(Language::Fortran),
        "glsl" => Some(Language::Glsl),
        "go" => Some(Language::Go),
        "graphql" => Some(Language::GraphQL),
        "groovy" => Some(Language::Groovy),
        "haskell" => Some(Language::Haskell),
        "hcl" => Some(Language::Hcl),
        "html" => Some(Language::Html),
        "ini" => Some(Language::Ini),
        "java" => Some(Language::Java),
        "javascript" => Some(Language::JavaScript),
        "json" => Some(Language::Json),
        "julia" => Some(Language::Julia),
        "kotlin" => Some(Language::Kotlin),
        "kustomize" => Some(Language::Kustomize),
        "lean" => Some(Language::Lean),
        "lua" => Some(Language::Lua),
        "magma" => Some(Language::Magma),
        "makefile" => Some(Language::Makefile),
        "markdown" => Some(Language::Markdown),
        "matlab" => Some(Language::Matlab),
        "meson" => Some(Language::Meson),
        "nix" => Some(Language::Nix),
        "ocaml" => Some(Language::OCaml),
        "perl" => Some(Language::Perl),
        "php" => Some(Language::Php),
        "protobuf" => Some(Language::Protobuf),
        "python" => Some(Language::Python),
        "r" => Some(Language::R),
        "ruby" => Some(Language::Ruby),
        "rust" => Some(Language::Rust),
        "scala" => Some(Language::Scala),
        "scss" => Some(Language::Scss),
        "sql" => Some(Language::Sql),
        "svelte" => Some(Language::Svelte),
        "swift" => Some(Language::Swift),
        "toml" => Some(Language::Toml),
        "tsx" => Some(Language::Tsx),
        "typescript" => Some(Language::TypeScript),
        "verilog" => Some(Language::Verilog),
        "vimscript" => Some(Language::VimScript),
        "vue" => Some(Language::Vue),
        "wolfram" => Some(Language::Wolfram),
        "xml" => Some(Language::Xml),
        "yaml" => Some(Language::Yaml),
        "zig" => Some(Language::Zig),
        // Phase 2 languages
        "ada" => Some(Language::Ada),
        "astro" => Some(Language::Astro),
        "awk" => Some(Language::Awk),
        "crystal" => Some(Language::Crystal),
        "d" | "dlang" => Some(Language::DLang),
        "fennel" => Some(Language::Fennel),
        "fish" => Some(Language::Fish),
        "gdscript" => Some(Language::GDScript),
        "gleam" => Some(Language::Gleam),
        "hare" => Some(Language::Hare),
        "janet" => Some(Language::Janet),
        "json5" => Some(Language::Json5),
        "jsonnet" => Some(Language::Jsonnet),
        "just" => Some(Language::Just),
        "kdl" => Some(Language::Kdl),
        "liquid" => Some(Language::Liquid),
        "luau" => Some(Language::Luau),
        "move" => Some(Language::Move),
        "nim" => Some(Language::Nim),
        "objective-c" | "objectivec" => Some(Language::ObjectiveC),
        "odin" => Some(Language::Odin),
        "pascal" => Some(Language::Pascal),
        "starlark" => Some(Language::Starlark),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_language_name_case_insensitive() {
        assert_eq!(parse_language_name("Rust"), Some(Language::Rust));
        assert_eq!(parse_language_name("rust"), Some(Language::Rust));
        assert_eq!(parse_language_name("RUST"), Some(Language::Rust));
        assert_eq!(parse_language_name("Python"), Some(Language::Python));
        assert_eq!(
            parse_language_name("TypeScript"),
            Some(Language::TypeScript)
        );
        assert_eq!(
            parse_language_name("javascript"),
            Some(Language::JavaScript)
        );
    }

    #[test]
    fn test_parse_language_name_aliases() {
        assert_eq!(parse_language_name("C++"), Some(Language::Cpp));
        assert_eq!(parse_language_name("cpp"), Some(Language::Cpp));
        assert_eq!(parse_language_name("C#"), Some(Language::CSharp));
        assert_eq!(parse_language_name("csharp"), Some(Language::CSharp));
        assert_eq!(parse_language_name("F#"), Some(Language::FSharp));
        assert_eq!(parse_language_name("fsharp"), Some(Language::FSharp));
        assert_eq!(parse_language_name("D"), Some(Language::DLang));
        assert_eq!(parse_language_name("dlang"), Some(Language::DLang));
        assert_eq!(
            parse_language_name("Objective-C"),
            Some(Language::ObjectiveC)
        );
        assert_eq!(
            parse_language_name("objectivec"),
            Some(Language::ObjectiveC)
        );
    }

    #[test]
    fn test_parse_language_name_unknown() {
        assert_eq!(parse_language_name("nonexistent"), None);
        assert_eq!(parse_language_name(""), None);
        assert_eq!(parse_language_name("foobar"), None);
    }

    #[test]
    fn test_parse_language_name_phase2_languages() {
        assert_eq!(parse_language_name("Ada"), Some(Language::Ada));
        assert_eq!(parse_language_name("Gleam"), Some(Language::Gleam));
        assert_eq!(parse_language_name("Nim"), Some(Language::Nim));
        assert_eq!(parse_language_name("Starlark"), Some(Language::Starlark));
        assert_eq!(parse_language_name("GDScript"), Some(Language::GDScript));
        assert_eq!(parse_language_name("KDL"), Some(Language::Kdl));
    }

    #[test]
    fn test_load_mappings_from_file_valid() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(
            f,
            r#"{{"language_mappings": {{"pyx": "Python", "mjs": "JavaScript"}}}}"#
        )
        .unwrap();

        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(&config_path, &mut mappings);

        assert_eq!(mappings.get("pyx"), Some(&Language::Python));
        assert_eq!(mappings.get("mjs"), Some(&Language::JavaScript));
    }

    #[test]
    fn test_load_mappings_from_file_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "not valid json {{{").unwrap();

        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(&config_path, &mut mappings);

        // Should be empty — invalid JSON is skipped with a warning
        assert!(mappings.is_empty());
    }

    #[test]
    fn test_load_mappings_from_file_unrecognized_language() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let mut f = std::fs::File::create(&config_path).unwrap();
        writeln!(
            f,
            r#"{{"language_mappings": {{"xyz": "NonexistentLang", "rs": "Rust"}}}}"#
        )
        .unwrap();

        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(&config_path, &mut mappings);

        // Only valid mapping should be present
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings.get("rs"), Some(&Language::Rust));
        assert!(!mappings.contains_key("xyz"));
    }

    #[test]
    fn test_load_mappings_from_file_missing_file() {
        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(Path::new("/tmp/nonexistent_config_xyz.json"), &mut mappings);
        assert!(mappings.is_empty());
    }

    #[test]
    fn test_load_language_mappings_project_overrides_global() {
        let dir = tempfile::tempdir().unwrap();
        let project_config = dir.path().join(".codebase-memory.json");
        let mut f = std::fs::File::create(&project_config).unwrap();
        writeln!(f, r#"{{"language_mappings": {{"pyx": "Rust"}}}}"#).unwrap();

        let mappings = load_language_mappings(dir.path());
        // Project-level mapping should be present
        assert_eq!(mappings.get("pyx"), Some(&Language::Rust));
    }

    #[test]
    fn test_load_language_mappings_no_configs() {
        let dir = tempfile::tempdir().unwrap();
        let mappings = load_language_mappings(dir.path());
        assert!(mappings.is_empty());
    }

    // --- Task 2.4: Additional tests for Req 9.1, 9.2, 9.3, 9.5 ---

    #[test]
    fn test_global_config_path_structure() {
        // Req 9.1: global config should be under config_dir/codebase-memory-mcp/config.json
        if let Some(path) = global_config_path() {
            assert!(
                path.ends_with("codebase-memory-mcp/config.json")
                    || path.ends_with("codebase-memory-mcp\\config.json")
            );
        }
        // If dirs::config_dir() returns None (e.g. in some CI), global_config_path() returns None — that's fine
    }

    #[test]
    fn test_project_config_override_global_same_extension() {
        // Req 9.3: project-level mapping overrides global for the same extension
        let dir = tempfile::tempdir().unwrap();

        // Simulate global config: "pyx" → Python
        let global_path = dir.path().join("global_config.json");
        let mut f = std::fs::File::create(&global_path).unwrap();
        writeln!(
            f,
            r#"{{"language_mappings": {{"pyx": "Python", "mjs": "JavaScript"}}}}"#
        )
        .unwrap();

        // Simulate project config: "pyx" → Rust (overrides global)
        let project_path = dir.path().join("project_config.json");
        let mut f2 = std::fs::File::create(&project_path).unwrap();
        writeln!(f2, r#"{{"language_mappings": {{"pyx": "Rust"}}}}"#).unwrap();

        // Load global first, then project (same order as load_language_mappings)
        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(&global_path, &mut mappings);
        assert_eq!(mappings.get("pyx"), Some(&Language::Python));
        assert_eq!(mappings.get("mjs"), Some(&Language::JavaScript));

        load_mappings_from_file(&project_path, &mut mappings);
        // Project overrides "pyx" to Rust; "mjs" from global survives
        assert_eq!(mappings.get("pyx"), Some(&Language::Rust));
        assert_eq!(mappings.get("mjs"), Some(&Language::JavaScript));
    }

    #[test]
    fn test_invalid_project_json_preserves_global_mappings() {
        // Req 9.5: invalid project JSON logs warning, global mappings survive
        let dir = tempfile::tempdir().unwrap();

        let global_path = dir.path().join("global_config.json");
        let mut f = std::fs::File::create(&global_path).unwrap();
        writeln!(f, r#"{{"language_mappings": {{"pyx": "Python"}}}}"#).unwrap();

        let project_path = dir.path().join("project_config.json");
        std::fs::write(&project_path, "not valid json {{{").unwrap();

        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(&global_path, &mut mappings);
        load_mappings_from_file(&project_path, &mut mappings);

        // Global mapping should survive despite invalid project config
        assert_eq!(mappings.get("pyx"), Some(&Language::Python));
    }

    #[test]
    fn test_load_mappings_no_language_mappings_field() {
        // Req 9.5: valid JSON but missing "language_mappings" field → no mappings loaded
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, r#"{"other_field": "value"}"#).unwrap();

        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(&config_path, &mut mappings);
        assert!(mappings.is_empty());
    }

    #[test]
    fn test_load_mappings_non_string_language_value() {
        // Req 9.5: language_mappings with non-string value → skip that entry
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"language_mappings": {"pyx": 42, "rs": "Rust"}}"#,
        )
        .unwrap();

        let mut mappings = LanguageMappings::new();
        load_mappings_from_file(&config_path, &mut mappings);

        // Only valid string mapping should be present
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings.get("rs"), Some(&Language::Rust));
    }
}
