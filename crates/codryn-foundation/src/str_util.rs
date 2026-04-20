/// Normalize path separators to forward slash.
pub fn normalize_path_sep(path: &str) -> String {
    path.replace('\\', "/")
}

/// Strip file extension from a path string.
pub fn strip_extension(path: &str) -> &str {
    let last_slash = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    match path[last_slash..].rfind('.') {
        Some(dot) => &path[..last_slash + dot],
        None => path,
    }
}

/// Truncate a string to at most `max` bytes on a char boundary.
pub fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Normalize a string for fuzzy matching: strip separators, lowercase.
/// `"travel-request"` → `"travelrequest"`, `"TravelRequestController"` → `"travelrequestcontroller"`
pub fn normalize_for_matching(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '-' && *c != '_' && *c != '.' && *c != '/' && *c != '{' && *c != '}')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Split a string into tokens by case boundaries and separators.
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if c == '-' || c == '_' || c == '.' || c == '/' || c == ' ' {
            if !current.is_empty() {
                tokens.push(current.to_lowercase());
                current.clear();
            }
        } else if c.is_uppercase() && !current.is_empty() {
            tokens.push(current.to_lowercase());
            current.clear();
            current.push(c);
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current.to_lowercase());
    }
    tokens
}

/// Compute Jaccard similarity between token sets of two strings.
pub fn token_similarity(a: &str, b: &str) -> f64 {
    let ta: std::collections::HashSet<String> = tokenize(a).into_iter().collect();
    let tb: std::collections::HashSet<String> = tokenize(b).into_iter().collect();
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let intersection = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    intersection / union
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize() {
        assert_eq!(normalize_path_sep("a\\b\\c"), "a/b/c");
        assert_eq!(normalize_path_sep("a/b/c"), "a/b/c");
    }

    #[test]
    fn test_strip_ext() {
        assert_eq!(strip_extension("src/main.rs"), "src/main");
        assert_eq!(strip_extension("noext"), "noext");
        assert_eq!(strip_extension("a/b.c/d.txt"), "a/b.c/d");
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 3), "hel");
        assert_eq!(truncate("hi", 10), "hi");
    }

    #[test]
    fn test_normalize_for_matching() {
        assert_eq!(normalize_for_matching("travel-request"), "travelrequest");
        assert_eq!(
            normalize_for_matching("TravelRequestController"),
            "travelrequestcontroller"
        );
        assert_eq!(
            normalize_for_matching("/v1/travelrequest/{id}"),
            "v1travelrequestid"
        );
        assert_eq!(
            normalize_for_matching("travel_request_service"),
            "travelrequestservice"
        );
    }

    #[test]
    fn test_token_similarity() {
        let s = token_similarity("travel-request", "TravelRequestController");
        assert!(s > 0.5, "expected > 0.5, got {}", s);
        let s2 = token_similarity("travel-request", "travelRequestService");
        assert!(s2 > 0.5, "expected > 0.5, got {}", s2);
        let s3 = token_similarity("travel-request", "UserController");
        assert!(s3 < 0.3, "expected < 0.3, got {}", s3);
    }

    #[test]
    fn test_tokenize() {
        assert_eq!(
            tokenize("TravelRequestController"),
            vec!["travel", "request", "controller"]
        );
        assert_eq!(tokenize("travel-request"), vec!["travel", "request"]);
        assert_eq!(
            tokenize("travel_request_service"),
            vec!["travel", "request", "service"]
        );
    }
}
