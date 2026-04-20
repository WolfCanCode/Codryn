use crate::str_util::normalize_path_sep;

/// Compute fully qualified name: project.dir.parts.name
pub fn fqn_compute(project: &str, rel_path: &str, name: Option<&str>) -> String {
    let path = normalize_path_sep(rel_path);
    let stripped = crate::str_util::strip_extension(&path);

    let mut segments: Vec<&str> = vec![project];
    for part in stripped.split('/').filter(|s| !s.is_empty()) {
        segments.push(part);
    }

    // Strip __init__ / index when a name is provided
    if let Some(n) = name {
        if !n.is_empty() {
            if let Some(last) = segments.last() {
                if *last == "__init__" || *last == "index" {
                    segments.pop();
                }
            }
        }
        segments.push(n);
    }

    segments.join(".")
}

/// Module QN: project.dir.parts (no name).
pub fn fqn_module(project: &str, rel_path: &str) -> String {
    fqn_compute(project, rel_path, None)
}

/// Folder QN: project.dir.parts from a directory path.
pub fn fqn_folder(project: &str, rel_dir: &str) -> String {
    let dir = normalize_path_sep(rel_dir);
    let mut segments: Vec<&str> = vec![project];
    for part in dir.split('/').filter(|s| !s.is_empty()) {
        segments.push(part);
    }
    segments.join(".")
}

/// Derive project name from absolute path — uses the last directory name.
pub fn project_name_from_path(abs_path: &str) -> String {
    if abs_path.is_empty() {
        return "root".into();
    }
    let normalized = normalize_path_sep(abs_path);
    let trimmed = normalized.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("root")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fqn_compute() {
        assert_eq!(
            fqn_compute("myproj", "src/main.rs", Some("main")),
            "myproj.src.main.main"
        );
        assert_eq!(
            fqn_compute("p", "lib/__init__.py", Some("Foo")),
            "p.lib.Foo"
        );
        assert_eq!(
            fqn_compute("p", "src/index.ts", Some("handler")),
            "p.src.handler"
        );
    }

    #[test]
    fn test_fqn_module() {
        assert_eq!(fqn_module("p", "src/utils.py"), "p.src.utils");
    }

    #[test]
    fn test_project_name() {
        assert_eq!(
            project_name_from_path("/home/user/my-project"),
            "my-project"
        );
        assert_eq!(
            project_name_from_path("/Users/taaleto7/Documents/work/mcp-tools/codryn"),
            "codryn"
        );
        assert_eq!(project_name_from_path(""), "root");
        assert_eq!(project_name_from_path("/"), "root");
    }
}
