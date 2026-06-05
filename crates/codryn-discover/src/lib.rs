mod language;
pub mod userconfig;

pub use language::detect as detect_language;
pub use language::detect_with_mappings;
pub use language::disambiguate_m_file;
pub use language::Language;
pub use userconfig::{load_language_mappings, parse_language_name, LanguageMappings};

use anyhow::Result;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DiscoveredFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub language: Language,
}

/// Directories that should always be excluded from indexing, regardless of .gitignore.
///
/// IMPORTANT: Only include names that are **unambiguously** dependency, cache, or
/// build-output directories. Generic names like `dist`, `build`, `out`, `vendor`,
/// `env`, `target`, `tmp` are intentionally omitted because they can be legitimate
/// source directories in many projects. Those should be handled by .gitignore instead.
const EXCLUDED_DIRS: &[&str] = &[
    // Version control
    ".git",
    ".hg",
    ".svn",
    // JavaScript / Node
    "node_modules",
    "bower_components",
    // Angular / Next / Nuxt / Svelte caches
    ".angular",
    ".next",
    ".nuxt",
    ".svelte-kit",
    // Python caches
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    // Python virtual environments (dotfile-prefixed are safe)
    ".venv",
    // Java / Gradle / Maven
    ".gradle",
    // IDE directories
    ".idea",
    ".vs",
    // Generic caches (dotfile-prefixed are safe)
    ".cache",
    ".parcel-cache",
    ".turbo",
    // Test coverage output
    ".nyc_output",
    // Documentation generators
    ".docusaurus",
    // Infrastructure
    ".terraform",
    ".serverless",
    ".aws-sam",
    "cdk.out",
    // Dart / Flutter
    ".dart_tool",
    ".pub-cache",
    // iOS / macOS
    "Pods",
    "DerivedData",
    // Ruby
    ".bundle",
];

/// Check if a relative path contains any excluded directory segment.
fn is_excluded(rel_path: &str) -> bool {
    for segment in rel_path.split('/') {
        for excluded in EXCLUDED_DIRS {
            if segment == *excluded {
                return true;
            }
        }
    }
    false
}

/// CI/CD configuration files that should always be indexed even if gitignored.
/// Some teams gitignore these (generated or externally managed), but a code
/// intelligence tool needs them for pipeline detection.
const CI_CD_FILES: &[&str] = &[
    ".gitlab-ci.yml",
    ".circleci/config.yml",
    "azure-pipelines.yml",
    "bitbucket-pipelines.yml",
    "Jenkinsfile",
];

/// Walk a repository directory, respecting .gitignore, and detect languages.
/// Also applies hardcoded exclusions for common dependency/build/cache directories
/// that should never be indexed even if .gitignore is missing or incomplete.
pub fn discover_files(root: &Path) -> Result<Vec<DiscoveredFile>> {
    discover_files_with_mappings(root, &userconfig::LanguageMappings::new())
}

/// Helper: collect just the relative paths from discovered files (sorted for deterministic assertions).
#[cfg(test)]
fn sorted_rel_paths(files: &[DiscoveredFile]) -> Vec<&str> {
    let mut paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
    paths.sort();
    paths
}

/// Walk a repository directory with user-configurable language mappings.
/// User mappings are checked first; falls back to built-in detection.
pub fn discover_files_with_mappings(
    root: &Path,
    mappings: &userconfig::LanguageMappings,
) -> Result<Vec<DiscoveredFile>> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| {
            // Skip excluded directories early so we don't even descend into them
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    return !EXCLUDED_DIRS.contains(&name);
                }
            }
            true
        })
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let rel = abs
            .strip_prefix(root)
            .unwrap_or(&abs)
            .to_string_lossy()
            .replace('\\', "/");

        // Double-check: skip files inside excluded directories
        // (filter_entry should catch most, but this handles edge cases)
        if is_excluded(&rel) {
            continue;
        }

        let mut lang = if mappings.is_empty() {
            language::detect(&rel)
        } else {
            language::detect_with_mappings(&rel, mappings)
        };
        if lang != Language::Unknown {
            // Override language for .m files using content-based disambiguation
            if rel.ends_with(".m") {
                lang = language::disambiguate_m_file(&abs);
            }
            files.push(DiscoveredFile {
                abs_path: abs,
                rel_path: rel,
                language: lang,
            });
        }
    }

    // Second pass: ensure critical CI/CD files are included even if gitignored.
    // Some projects gitignore their CI configs (e.g. generated or externally managed),
    // but a code intelligence tool still needs to index them for pipeline detection.
    let seen: std::collections::HashSet<String> =
        files.iter().map(|f| f.rel_path.clone()).collect();
    for ci_path in CI_CD_FILES {
        if seen.contains(*ci_path) {
            continue;
        }
        let abs = root.join(ci_path);
        if abs.is_file() {
            let lang = if mappings.is_empty() {
                language::detect(ci_path)
            } else {
                language::detect_with_mappings(ci_path, mappings)
            };
            if lang != Language::Unknown {
                files.push(DiscoveredFile {
                    abs_path: abs,
                    rel_path: ci_path.to_string(),
                    language: lang,
                });
            }
        }
    }
    // Also scan for GitHub Actions workflows which may be gitignored
    let gh_workflows = root.join(".github/workflows");
    if gh_workflows.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&gh_workflows) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if seen.contains(&rel) {
                    continue;
                }
                let lang = if mappings.is_empty() {
                    language::detect(&rel)
                } else {
                    language::detect_with_mappings(&rel, mappings)
                };
                if lang != Language::Unknown {
                    files.push(DiscoveredFile {
                        abs_path: path,
                        rel_path: rel,
                        language: lang,
                    });
                }
            }
        }
    }

    Ok(files)
}

#[cfg(test)]
mod gitignore_tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    /// Create a temp directory, write the given files, and `git init` so the
    /// `ignore` crate's WalkBuilder respects `.gitignore` rules.
    fn setup_repo(structure: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (path, content) in structure {
            let full = dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, content).unwrap();
        }
        // git init is required for the ignore crate to honour .gitignore
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir.path())
            .status()
            .expect("git must be available");
        assert!(status.success(), "git init failed");
        dir
    }

    // ── Requirement 15.1: ** glob patterns ──────────────────────────────

    #[test]
    fn test_double_star_glob_matches_zero_or_more_dirs() {
        let dir = setup_repo(&[
            (".gitignore", "**/ignored.py"),
            ("ignored.py", "x"),
            ("a/ignored.py", "x"),
            ("a/b/ignored.py", "x"),
            ("keep.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.iter().all(|p| !p.contains("ignored")),
            "** should exclude at every depth, got: {:?}",
            paths
        );
        assert!(paths.contains(&"keep.py"));
    }

    #[test]
    fn test_double_star_in_middle_of_pattern() {
        // Pattern: src/**/test.py  should match src/test.py, src/a/test.py, etc.
        let dir = setup_repo(&[
            (".gitignore", "src/**/test.py"),
            ("src/test.py", "x"),
            ("src/a/test.py", "x"),
            ("src/a/b/test.py", "x"),
            ("test.py", "x"),     // NOT under src/ — should be kept
            ("src/main.py", "x"), // different name — should be kept
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(!paths.contains(&"src/test.py"));
        assert!(!paths.contains(&"src/a/test.py"));
        assert!(!paths.contains(&"src/a/b/test.py"));
        assert!(paths.contains(&"test.py"));
        assert!(paths.contains(&"src/main.py"));
    }

    // ── Requirement 15.2: ? wildcard ────────────────────────────────────

    #[test]
    fn test_question_mark_matches_exactly_one_char() {
        let dir = setup_repo(&[
            (".gitignore", "?.py"),
            ("a.py", "x"),   // single char — excluded
            ("ab.py", "x"),  // two chars — kept
            ("abc.py", "x"), // three chars — kept
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            !paths.contains(&"a.py"),
            "?.py should match single-char name"
        );
        assert!(paths.contains(&"ab.py"));
        assert!(paths.contains(&"abc.py"));
    }

    #[test]
    fn test_question_mark_in_middle() {
        let dir = setup_repo(&[
            (".gitignore", "te?t.py"),
            ("test.py", "x"),  // matches te?t.py
            ("text.py", "x"),  // matches te?t.py
            ("teest.py", "x"), // does NOT match (two chars in ? position)
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(!paths.contains(&"test.py"));
        assert!(!paths.contains(&"text.py"));
        assert!(paths.contains(&"teest.py"));
    }

    // ── Requirement 15.3: character class patterns ──────────────────────

    #[test]
    fn test_character_class_basic() {
        let dir = setup_repo(&[
            (".gitignore", "[abc].py"),
            ("a.py", "x"), // matches
            ("b.py", "x"), // matches
            ("c.py", "x"), // matches
            ("d.py", "x"), // does NOT match
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(!paths.contains(&"a.py"));
        assert!(!paths.contains(&"b.py"));
        assert!(!paths.contains(&"c.py"));
        assert!(paths.contains(&"d.py"));
    }

    #[test]
    fn test_character_class_range() {
        let dir = setup_repo(&[
            (".gitignore", "[a-c].py"),
            ("a.py", "x"),
            ("b.py", "x"),
            ("c.py", "x"),
            ("d.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(!paths.contains(&"a.py"));
        assert!(!paths.contains(&"b.py"));
        assert!(!paths.contains(&"c.py"));
        assert!(paths.contains(&"d.py"));
    }

    #[test]
    fn test_character_class_negation() {
        // [!abc].py should exclude everything EXCEPT a, b, c
        let dir = setup_repo(&[
            (".gitignore", "[!abc].py"),
            ("a.py", "x"), // NOT excluded (negated class)
            ("d.py", "x"), // excluded
            ("z.py", "x"), // excluded
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(paths.contains(&"a.py"), "[!abc] should NOT match a");
        assert!(!paths.contains(&"d.py"), "[!abc] should match d");
        assert!(!paths.contains(&"z.py"), "[!abc] should match z");
    }

    // ── Requirement 15.4: negation patterns (! prefix) ─────────────────

    #[test]
    fn test_negation_re_includes_excluded_file() {
        let dir = setup_repo(&[
            (".gitignore", "*.py\n!keep.py"),
            ("ignore.py", "x"),
            ("keep.py", "x"),
            ("also_ignore.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(!paths.contains(&"ignore.py"));
        assert!(!paths.contains(&"also_ignore.py"));
        assert!(paths.contains(&"keep.py"), "!keep.py should re-include it");
    }

    #[test]
    fn test_negation_with_directory_pattern() {
        // Exclude everything in build/, but re-include build/output.py
        let dir = setup_repo(&[
            (".gitignore", "build/*\n!build/output.py"),
            ("build/output.py", "x"),
            ("build/temp.py", "x"),
            ("src/main.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.contains(&"build/output.py"),
            "negation should re-include build/output.py"
        );
        assert!(!paths.contains(&"build/temp.py"));
        assert!(paths.contains(&"src/main.py"));
    }

    // ── Requirement 15.5: directory-only patterns (trailing /) ──────────

    #[test]
    fn test_directory_only_pattern_excludes_dir_not_file() {
        let dir = setup_repo(&[
            (".gitignore", "logs/"),
            ("logs/app.py", "x"),
            ("logs/debug.py", "x"),
            ("logs.py", "x"), // file named "logs.py" — should be kept
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.iter().all(|p| !p.starts_with("logs/")),
            "logs/ pattern should exclude the directory contents"
        );
        assert!(
            paths.contains(&"logs.py"),
            "logs/ should not exclude the file logs.py"
        );
    }

    #[test]
    fn test_directory_only_pattern_nested() {
        let dir = setup_repo(&[
            (".gitignore", "tmp/"),
            ("tmp/cache.py", "x"),
            ("src/tmp/cache.py", "x"), // nested tmp/ should also be excluded
            ("src/main.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(!paths.contains(&"tmp/cache.py"));
        assert!(!paths.contains(&"src/tmp/cache.py"));
        assert!(paths.contains(&"src/main.py"));
    }

    // ── Requirement 15.6: nested .gitignore precedence ──────────────────

    #[test]
    fn test_nested_gitignore_overrides_parent() {
        // Root ignores *.pyc, but sub/.gitignore re-includes important.pyc
        // We use .py extension since .pyc is not a recognized language.
        // Instead: root ignores all .py in generated/, sub re-includes one.
        let dir = setup_repo(&[
            (".gitignore", "generated/*.py"),
            ("generated/.gitignore", "!keep.py"),
            ("generated/keep.py", "x"),
            ("generated/drop.py", "x"),
            ("src/main.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.contains(&"generated/keep.py"),
            "nested .gitignore should re-include keep.py, got: {:?}",
            paths
        );
        assert!(!paths.contains(&"generated/drop.py"));
        assert!(paths.contains(&"src/main.py"));
    }

    #[test]
    fn test_deeper_gitignore_takes_precedence() {
        // Root: ignore *.py in tests/
        // tests/.gitignore: re-include conftest.py
        // tests/unit/.gitignore: ignore conftest.py again
        let dir = setup_repo(&[
            (".gitignore", "tests/**/*.py"),
            ("tests/.gitignore", "!conftest.py\n!unit/"),
            ("tests/unit/.gitignore", "conftest.py"),
            ("tests/conftest.py", "x"),
            ("tests/unit/conftest.py", "x"),
            ("tests/unit/test_a.py", "x"),
            ("src/main.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        // tests/conftest.py should be re-included by tests/.gitignore
        assert!(
            paths.contains(&"tests/conftest.py"),
            "tests/.gitignore should re-include conftest.py, got: {:?}",
            paths
        );
        // tests/unit/conftest.py should be re-excluded by tests/unit/.gitignore
        assert!(
            !paths.contains(&"tests/unit/conftest.py"),
            "tests/unit/.gitignore should re-exclude conftest.py"
        );
        assert!(paths.contains(&"src/main.py"));
    }

    // ── Requirement 15.7: tracked files with recognized extensions ──────

    #[test]
    fn test_unignored_files_with_recognized_extensions_are_discovered() {
        let dir = setup_repo(&[
            ("main.py", "print('hello')"),
            ("lib.rs", "fn main() {}"),
            ("app.js", "console.log('hi')"),
            ("style.css", "body {}"),
            ("readme.txt", "hello"), // .txt is not a recognized language
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(paths.contains(&"main.py"));
        assert!(paths.contains(&"lib.rs"));
        assert!(paths.contains(&"app.js"));
        assert!(paths.contains(&"style.css"));
        // .txt is not a recognized language extension, so it should not appear
        assert!(!paths.contains(&"readme.txt"));
    }

    // ── Combined / edge-case tests ──────────────────────────────────────

    #[test]
    fn test_combined_patterns() {
        let dir = setup_repo(&[
            (
                ".gitignore",
                "# comment line\n*.pyc\nbuild/\n!build/keep.py\n[Tt]emp.py\n",
            ),
            ("app.py", "x"),
            ("build/artifact.py", "x"),
            ("build/keep.py", "x"),
            ("Temp.py", "x"),
            ("temp.py", "x"),
            ("other.py", "x"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(paths.contains(&"app.py"));
        assert!(paths.contains(&"other.py"));
        // build/ directory pattern excludes everything inside
        // Note: build/ as a directory-only pattern means the whole dir is ignored,
        // so the negation !build/keep.py may not work because the directory itself
        // is never entered. This is standard git behavior.
        assert!(!paths.contains(&"build/artifact.py"));
        // Character class [Tt]emp.py
        assert!(!paths.contains(&"Temp.py"));
        assert!(!paths.contains(&"temp.py"));
    }

    // ── CI/CD dotfile discovery ─────────────────────────────────────────

    #[test]
    fn test_github_workflows_discovered() {
        let dir = setup_repo(&[
            (".github/workflows/ci.yml", "name: CI\non: push\njobs: {}"),
            (
                ".github/workflows/release.yaml",
                "name: Release\non: push\njobs: {}",
            ),
            ("src/main.py", "print('hello')"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.contains(&".github/workflows/ci.yml"),
            ".github/workflows should be discovered, got: {:?}",
            paths
        );
        assert!(
            paths.contains(&".github/workflows/release.yaml"),
            ".github/workflows yaml should be discovered, got: {:?}",
            paths
        );
        assert!(paths.contains(&"src/main.py"));
    }

    #[test]
    fn test_gitlab_ci_discovered() {
        let dir = setup_repo(&[
            (".gitlab-ci.yml", "stages:\n  - build\n  - test"),
            ("src/main.py", "print('hello')"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.contains(&".gitlab-ci.yml"),
            ".gitlab-ci.yml should be discovered, got: {:?}",
            paths
        );
        assert!(paths.contains(&"src/main.py"));
    }

    #[test]
    fn test_circleci_config_discovered() {
        let dir = setup_repo(&[
            (".circleci/config.yml", "version: 2.1\njobs: {}"),
            ("src/main.py", "print('hello')"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.contains(&".circleci/config.yml"),
            ".circleci/config.yml should be discovered, got: {:?}",
            paths
        );
    }

    #[test]
    fn test_excluded_dotdirs_still_excluded() {
        // Ensure that enabling hidden file discovery doesn't accidentally
        // include directories we explicitly exclude (caches, VCS, etc.)
        let dir = setup_repo(&[
            (".venv/lib/site.py", "x"),
            (".gradle/caches/build.gradle", "x"),
            (".idea/workspace.xml", "x"),
            (".github/workflows/ci.yml", "name: CI\non: push\njobs: {}"),
            ("src/main.py", "print('hello')"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(!paths.iter().any(|p| p.starts_with(".venv/")));
        assert!(!paths.iter().any(|p| p.starts_with(".gradle/")));
        assert!(!paths.iter().any(|p| p.starts_with(".idea/")));
        assert!(!paths.iter().any(|p| p.starts_with(".git/")));
        // But CI/CD files should be discovered
        assert!(paths.contains(&".github/workflows/ci.yml"));
        assert!(paths.contains(&"src/main.py"));
    }

    #[test]
    fn test_gitignored_ci_files_still_discovered() {
        // CI/CD files should be discovered even if the project's .gitignore excludes them
        let dir = setup_repo(&[
            (".gitignore", ".gitlab-ci.yml\n.circleci/"),
            (".gitlab-ci.yml", "stages:\n  - build\n  - test"),
            (".circleci/config.yml", "version: 2.1\njobs: {}"),
            (".github/workflows/ci.yml", "name: CI\non: push\njobs: {}"),
            ("src/main.py", "print('hello')"),
        ]);
        let files = discover_files(dir.path()).unwrap();
        let paths = sorted_rel_paths(&files);
        assert!(
            paths.contains(&".gitlab-ci.yml"),
            "gitignored .gitlab-ci.yml should still be discovered, got: {:?}",
            paths
        );
        assert!(
            paths.contains(&".github/workflows/ci.yml"),
            ".github/workflows should be discovered, got: {:?}",
            paths
        );
        assert!(paths.contains(&"src/main.py"));
    }
}
