use std::collections::HashMap;

/// Symbol registry for cross-file resolution.
/// Maps short name -> list of (qualified_name, file_path).
pub struct Registry {
    by_name: HashMap<String, Vec<RegistryEntry>>,
}

#[derive(Debug, Clone)]
pub struct RegistryEntry {
    pub qualified_name: String,
    pub file_path: String,
    pub label: String,
    pub start_line: i32,
    pub end_line: i32,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        name: &str,
        qn: &str,
        file_path: &str,
        label: &str,
        start_line: i32,
        end_line: i32,
    ) {
        self.by_name
            .entry(name.to_owned())
            .or_default()
            .push(RegistryEntry {
                qualified_name: qn.to_owned(),
                file_path: file_path.to_owned(),
                label: label.to_owned(),
                start_line,
                end_line,
            });
    }

    pub fn lookup(&self, name: &str) -> &[RegistryEntry] {
        self.by_name.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn all_names(&self) -> Vec<&str> {
        self.by_name.keys().map(|s| s.as_str()).collect()
    }

    /// Get all entries for a given file, sorted by start_line.
    pub fn entries_for_file(&self, file_path: &str) -> Vec<&RegistryEntry> {
        let mut out: Vec<&RegistryEntry> = self
            .by_name
            .values()
            .flatten()
            .filter(|e| e.file_path == file_path)
            .collect();
        out.sort_by_key(|e| e.start_line);
        out
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}
