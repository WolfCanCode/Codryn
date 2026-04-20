mod language;

pub use language::detect as detect_language;
pub use language::Language;

use anyhow::Result;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DiscoveredFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub language: Language,
}

/// Walk a repository directory, respecting .gitignore, and detect languages.
pub fn discover_files(root: &Path) -> Result<Vec<DiscoveredFile>> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
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

        let lang = language::detect(&rel);
        if lang != Language::Unknown {
            files.push(DiscoveredFile {
                abs_path: abs,
                rel_path: rel,
                language: lang,
            });
        }
    }
    Ok(files)
}
