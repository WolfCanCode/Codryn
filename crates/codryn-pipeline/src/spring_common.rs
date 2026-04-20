/// Maps Spring mapping annotation name to HTTP method. Returns None for RequestMapping (caller resolves).
pub fn resolve_http_method(annotation: &str) -> Option<&'static str> {
    match annotation {
        "GetMapping" => Some("GET"),
        "PostMapping" => Some("POST"),
        "PutMapping" => Some("PUT"),
        "PatchMapping" => Some("PATCH"),
        "DeleteMapping" => Some("DELETE"),
        _ => None,
    }
}

pub const SPRING_MAPPING_ANNOTATIONS: &[&str] = &[
    "GetMapping",
    "PostMapping",
    "PutMapping",
    "PatchMapping",
    "DeleteMapping",
    "RequestMapping",
];

pub const SPRING_CONTROLLER_ANNOTATIONS: &[&str] = &["RestController", "Controller"];

/// Combine class-level base path with method-level path.
pub fn combine_paths(base: &str, method_path: &str) -> String {
    let base = base.trim_end_matches('/');
    let method = method_path.trim_start_matches('/');
    if base.is_empty() && method.is_empty() {
        return "/".into();
    }
    if base.is_empty() {
        return format!("/{method}");
    }
    if method.is_empty() {
        return base.to_string();
    }
    format!("{base}/{method}")
}

const SKIP_TYPES: &[&str] = &[
    "String",
    "Object",
    "Map",
    "List",
    "Set",
    "Collection",
    "void",
    "Void",
    "Integer",
    "Long",
    "Boolean",
    "Double",
    "Float",
    "Short",
    "Byte",
    "Character",
    "int",
    "long",
    "boolean",
    "double",
    "float",
    "short",
    "byte",
    "char",
    "ResponseEntity",
    "Optional",
    "CompletableFuture",
    "Mono",
    "Flux",
];

/// Returns true if the type name is a real DTO candidate (not a primitive/wrapper/collection).
pub fn is_dto_candidate(type_name: &str) -> bool {
    !type_name.is_empty() && !SKIP_TYPES.contains(&type_name)
}

/// Extract inner type from generic wrappers: `ResponseEntity<UserDto>` → `UserDto`, `List<Foo>` → `Foo`.
pub fn unwrap_generic_type(type_text: &str) -> &str {
    if let Some(start) = type_text.find('<') {
        let inner = &type_text[start + 1..];
        if let Some(end) = inner.rfind('>') {
            let inner = inner[..end].trim();
            // Recurse for nested: ResponseEntity<List<Foo>> → List<Foo> → Foo
            let outer = &type_text[..start];
            if SKIP_TYPES.contains(&outer) {
                return unwrap_generic_type(inner);
            }
        }
    }
    type_text
}

/// Classify a symbol into an architectural layer.
/// Priority: annotations > name suffix > file path.
pub fn classify_layer(name: &str, annotations: &[&str], file_path: &str) -> Option<&'static str> {
    // 1. Annotation-based
    for ann in annotations {
        match *ann {
            "RestController" | "Controller" => return Some("controller"),
            "Service" => return Some("service"),
            "Repository" => return Some("repository"),
            "Entity" => return Some("entity"),
            "Configuration" | "Component" => return Some("config"),
            _ => {}
        }
    }
    // 2. Name suffix
    let n = name.to_lowercase();
    if n.ends_with("controller") || n.ends_with("resource") {
        return Some("controller");
    }
    if n.ends_with("service") || n.ends_with("serviceimpl") {
        return Some("service");
    }
    if n.ends_with("repository") || n.ends_with("repo") || n.ends_with("dao") {
        return Some("repository");
    }
    if n.ends_with("dto") || n.ends_with("request") || n.ends_with("response") {
        return Some("dto");
    }
    if n.ends_with("entity") {
        return Some("entity");
    }
    if n.ends_with("validator") {
        return Some("validator");
    }
    if n.ends_with("model") {
        return Some("model");
    }
    // 3. File path
    let fp = file_path.to_lowercase();
    if fp.contains("/controller/") || fp.contains("/api/") || fp.contains("/rest/") {
        return Some("controller");
    }
    if fp.contains("/service/") {
        return Some("service");
    }
    if fp.contains("/repository/") || fp.contains("/repo/") || fp.contains("/dao/") {
        return Some("repository");
    }
    if fp.contains("/dto/") {
        return Some("dto");
    }
    if fp.contains("/entity/") || fp.contains("/domain/") {
        return Some("entity");
    }
    if fp.contains("/model/") {
        return Some("model");
    }
    None
}

/// Extract the first string literal value from annotation text like `"/users"` or `value = "/users"`.
pub fn extract_annotation_string(text: &str) -> Option<String> {
    let start = text.find('"')?;
    let rest = &text[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Detect HTTP method from @RequestMapping arguments text.
pub fn request_mapping_method(args_text: &str) -> &'static str {
    let upper = args_text.to_uppercase();
    if upper.contains("POST") {
        "POST"
    } else if upper.contains("PUT") {
        "PUT"
    } else if upper.contains("PATCH") {
        "PATCH"
    } else if upper.contains("DELETE") {
        "DELETE"
    } else {
        "GET"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_http_method() {
        assert_eq!(resolve_http_method("GetMapping"), Some("GET"));
        assert_eq!(resolve_http_method("PostMapping"), Some("POST"));
        assert_eq!(resolve_http_method("PutMapping"), Some("PUT"));
        assert_eq!(resolve_http_method("PatchMapping"), Some("PATCH"));
        assert_eq!(resolve_http_method("DeleteMapping"), Some("DELETE"));
        assert_eq!(resolve_http_method("RequestMapping"), None);
        assert_eq!(resolve_http_method("Override"), None);
    }

    #[test]
    fn test_combine_paths() {
        assert_eq!(combine_paths("/api", "/users"), "/api/users");
        assert_eq!(combine_paths("/api/", "/users"), "/api/users");
        assert_eq!(combine_paths("/api", "users"), "/api/users");
        assert_eq!(combine_paths("", "/users"), "/users");
        assert_eq!(combine_paths("/api", ""), "/api");
        assert_eq!(combine_paths("", ""), "/");
    }

    #[test]
    fn test_classify_layer() {
        assert_eq!(
            classify_layer("UserController", &["RestController"], "src/Ctl.java"),
            Some("controller")
        );
        assert_eq!(
            classify_layer("UserService", &[], "src/svc/UserService.java"),
            Some("service")
        );
        assert_eq!(
            classify_layer("UserRepo", &["Repository"], "src/UserRepo.java"),
            Some("repository")
        );
        assert_eq!(classify_layer("Foo", &[], "src/dto/Foo.java"), Some("dto"));
        assert_eq!(
            classify_layer("CreateUserRequest", &[], "src/Req.java"),
            Some("dto")
        );
        assert_eq!(classify_layer("Unrelated", &[], "src/Unrelated.java"), None);
    }

    #[test]
    fn test_is_dto_candidate() {
        assert!(!is_dto_candidate("String"));
        assert!(!is_dto_candidate("void"));
        assert!(!is_dto_candidate("ResponseEntity"));
        assert!(!is_dto_candidate("List"));
        assert!(is_dto_candidate("UserDto"));
        assert!(is_dto_candidate("CreateUserRequest"));
    }

    #[test]
    fn test_unwrap_generic_type() {
        assert_eq!(unwrap_generic_type("ResponseEntity<UserDto>"), "UserDto");
        assert_eq!(unwrap_generic_type("List<UserDto>"), "UserDto");
        assert_eq!(unwrap_generic_type("Optional<UserDto>"), "UserDto");
        assert_eq!(unwrap_generic_type("UserDto"), "UserDto");
        assert_eq!(
            unwrap_generic_type("ResponseEntity<List<UserDto>>"),
            "UserDto"
        );
    }

    #[test]
    fn test_request_mapping_method() {
        assert_eq!(
            request_mapping_method("method = RequestMethod.POST"),
            "POST"
        );
        assert_eq!(request_mapping_method("value = \"/users\""), "GET");
    }
}
