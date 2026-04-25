/// Go-specific layer classification and helpers.
/// Classify a Go symbol into an architectural layer.
/// Priority: file path (package) > name suffix.
pub fn classify_go_layer(name: &str, file_path: &str) -> Option<&'static str> {
    let fp = file_path.to_lowercase();
    let fp = format!("/{fp}"); // normalize for contains checks
                               // Package/path-based
    if fp.contains("/handler/")
        || fp.contains("/handlers/")
        || fp.contains("/api/")
        || fp.contains("/rest/")
    {
        return Some("controller");
    }
    if fp.contains("/service/") || fp.contains("/services/") || fp.contains("/usecase/") {
        return Some("service");
    }
    if fp.contains("/repo/")
        || fp.contains("/repository/")
        || fp.contains("/store/")
        || fp.contains("/dao/")
        || fp.contains("/persistence/")
    {
        return Some("repository");
    }
    if fp.contains("/model/")
        || fp.contains("/models/")
        || fp.contains("/entity/")
        || fp.contains("/domain/")
        || fp.contains("/dto/")
    {
        return Some("model");
    }
    if fp.contains("/middleware/") || fp.contains("/interceptor/") {
        return Some("middleware");
    }
    if fp.contains("/config/") || fp.contains("/cfg/") {
        return Some("config");
    }
    if fp.contains("/cmd/") {
        return Some("entrypoint");
    }
    // Name suffix
    let n = name.to_lowercase();
    if n.ends_with("handler") || n.ends_with("controller") || n.ends_with("resource") {
        return Some("controller");
    }
    if n.ends_with("service") || n.ends_with("svc") || n.ends_with("usecase") {
        return Some("service");
    }
    if n.ends_with("repo")
        || n.ends_with("repository")
        || n.ends_with("store")
        || n.ends_with("dao")
    {
        return Some("repository");
    }
    if n.ends_with("model") || n.ends_with("dto") || n.ends_with("entity") {
        return Some("model");
    }
    if n.ends_with("middleware") {
        return Some("middleware");
    }
    None
}

/// HTTP method names for route method matching.
pub const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Map a framework method call name to an HTTP method.
/// Handles both upper (Gin/Echo: GET, POST) and title case (Chi/Fiber: Get, Post).
pub fn method_call_to_http(call: &str) -> Option<&'static str> {
    match call {
        "GET" | "Get" => Some("GET"),
        "POST" | "Post" => Some("POST"),
        "PUT" | "Put" => Some("PUT"),
        "PATCH" | "Patch" => Some("PATCH"),
        "DELETE" | "Delete" => Some("DELETE"),
        "HEAD" | "Head" => Some("HEAD"),
        "OPTIONS" | "Options" => Some("OPTIONS"),
        "Any" => Some("ANY"),
        _ => None,
    }
}

/// Known Go standard library packages (skip for import resolution).
pub fn is_stdlib_import(path: &str) -> bool {
    !path.contains('.') && !path.contains('/')
        || matches!(
            path.split('/').next().unwrap_or(""),
            "fmt"
                | "net"
                | "os"
                | "io"
                | "log"
                | "sync"
                | "time"
                | "math"
                | "sort"
                | "strings"
                | "strconv"
                | "bytes"
                | "errors"
                | "context"
                | "encoding"
                | "crypto"
                | "database"
                | "html"
                | "path"
                | "regexp"
                | "testing"
                | "reflect"
                | "runtime"
                | "syscall"
                | "unicode"
                | "bufio"
                | "archive"
                | "compress"
                | "container"
                | "debug"
                | "embed"
                | "expvar"
                | "flag"
                | "go"
                | "hash"
                | "image"
                | "index"
                | "internal"
                | "maps"
                | "mime"
                | "plugin"
                | "slices"
                | "text"
                | "unsafe"
        )
}

/// Detect Go framework from import path.
pub enum GoFramework {
    StdHttp,
    Gin,
    Echo,
    Chi,
    Fiber,
    GorillaMux,
    GoKit,
    Ginkgo,
}

pub fn detect_framework(import_path: &str) -> Option<GoFramework> {
    if import_path == "net/http" {
        return Some(GoFramework::StdHttp);
    }
    if import_path.contains("gin-gonic/gin") {
        return Some(GoFramework::Gin);
    }
    if import_path.contains("labstack/echo") {
        return Some(GoFramework::Echo);
    }
    if import_path.contains("go-chi/chi") {
        return Some(GoFramework::Chi);
    }
    if import_path.contains("gofiber/fiber") {
        return Some(GoFramework::Fiber);
    }
    if import_path.contains("gorilla/mux") {
        return Some(GoFramework::GorillaMux);
    }
    if import_path.contains("go-kit/kit") {
        return Some(GoFramework::GoKit);
    }
    if import_path.contains("onsi/ginkgo") || import_path.contains("onsi/gomega") {
        return Some(GoFramework::Ginkgo);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_go_layer() {
        assert_eq!(
            classify_go_layer("Handler", "handler/handler.go"),
            Some("controller")
        );
        assert_eq!(
            classify_go_layer("TaskService", "service/service.go"),
            Some("service")
        );
        assert_eq!(
            classify_go_layer("TaskRepo", "repo/repo.go"),
            Some("repository")
        );
        assert_eq!(classify_go_layer("Task", "models/models.go"), Some("model"));
        assert_eq!(classify_go_layer("main", "main.go"), None);
        // Name suffix fallback
        assert_eq!(
            classify_go_layer("UserHandler", "pkg/user.go"),
            Some("controller")
        );
        assert_eq!(
            classify_go_layer("OrderRepo", "pkg/order.go"),
            Some("repository")
        );
    }

    #[test]
    fn test_is_stdlib() {
        assert!(is_stdlib_import("fmt"));
        assert!(is_stdlib_import("net/http"));
        assert!(is_stdlib_import("encoding/json"));
        assert!(!is_stdlib_import("taskapi/handler"));
        assert!(!is_stdlib_import("github.com/gin-gonic/gin"));
    }

    #[test]
    fn test_method_call_to_http() {
        assert_eq!(method_call_to_http("GET"), Some("GET"));
        assert_eq!(method_call_to_http("Get"), Some("GET"));
        assert_eq!(method_call_to_http("Post"), Some("POST"));
        assert_eq!(method_call_to_http("Foo"), None);
    }
}
