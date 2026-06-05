//! Pipeline pass: Decorator and Annotation Tag extraction.
//!
//! Extracts @decorator/@Annotation patterns across Python, Java, Kotlin, and TypeScript.
//! Normalizes decorator names by stripping the leading `@`, stripping arguments, and
//! storing only the unqualified name. Stores as a `decorators` JSON array property on
//! nodes (max 50 entries, preserving declaration order).
//!
//! Requirements: 22.1, 22.2, 22.3, 22.4, 22.5, 22.6

use std::collections::HashMap;
use std::sync::LazyLock;

use codryn_discover::{DiscoveredFile, Language};
use codryn_store::Store;
use rayon::prelude::*;

use crate::FileCache;

// ── Decorator extraction patterns ─────────────────────────────────────────

/// Python decorator: `@decorator` or `@module.decorator` or `@decorator(args)`
/// Captures the full dotted name after @, we'll take the last segment (unqualified).
static PYTHON_DECORATOR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^[ \t]*@([\w.]+)").unwrap());

/// Java/Kotlin annotation: `@Annotation` or `@pkg.Annotation` or `@Annotation(args)`
/// Captures the full dotted name after @, we'll take the last segment (unqualified).
static JAVA_ANNOTATION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)@([\w.]+)").unwrap());

/// TypeScript decorator: `@Decorator` or `@Decorator(args)`
/// Captures the name after @, we'll take the last segment (unqualified).
static TS_DECORATOR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)@([\w.]+)").unwrap());

/// Maximum number of decorators stored per node.
pub const MAX_DECORATORS_PER_NODE: usize = 50;

// ── Main pass entry point ─────────────────────────────────────────────────

/// Extract decorators/annotations from source files and store them as a `decorators`
/// JSON array property on the corresponding graph nodes.
///
/// For each source file in a supported language (Python, Java, Kotlin, TypeScript):
/// 1. Extract all decorator/annotation occurrences with their line numbers
/// 2. Query the store for nodes in that file
/// 3. Associate decorators with the node they decorate (the node starting on or
///    immediately after the decorator lines)
/// 4. Update node properties with the `decorators` array
///
/// Nodes without decorators get an empty `decorators` list (Requirement 22.5).
pub fn pass_decorators(
    store: &Store,
    files: &[&DiscoveredFile],
    file_cache: &FileCache,
    project: &str,
) {
    // Filter to supported languages only
    let supported_files: Vec<&&DiscoveredFile> = files
        .iter()
        .filter(|f| is_decorator_language(f.language))
        .collect();

    if supported_files.is_empty() {
        return;
    }

    // Process files in parallel, collect (file_path, line -> decorators) mappings
    let file_decorators: Vec<(String, Vec<DecoratorOccurrence>)> = supported_files
        .par_iter()
        .filter_map(|f| {
            let source = if let Some(cached) = file_cache.get(&f.abs_path) {
                cached
            } else {
                match std::fs::read_to_string(&f.abs_path) {
                    Ok(s) => std::sync::Arc::new(s),
                    Err(_) => return None,
                }
            };

            let decorators = extract_decorators(&source, f.language);
            if decorators.is_empty() {
                // Still need to process this file to set empty decorators on nodes
                Some((f.rel_path.clone(), Vec::new()))
            } else {
                Some((f.rel_path.clone(), decorators))
            }
        })
        .collect();

    // Collect all property updates: (node_id, updated_properties_json)
    let mut updates: Vec<(i64, String)> = Vec::new();
    let mut annotated_count = 0usize;

    for (file_path, decorators) in &file_decorators {
        // Query the store for nodes in this file
        let nodes = match store.get_nodes_for_file(project, file_path) {
            Ok(n) => n,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    file = file_path.as_str(),
                    "pass_decorators: failed to query nodes for file, skipping"
                );
                continue;
            }
        };

        if nodes.is_empty() {
            continue;
        }

        // Associate decorators with nodes based on line proximity
        let node_decorators = associate_decorators_with_nodes(&nodes, decorators);

        // Update each node's properties with its decorators
        for node in &nodes {
            let decs = node_decorators.get(&node.id).cloned().unwrap_or_default();

            let updated_props = set_decorators_property(node.properties_json.as_deref(), &decs);
            updates.push((node.id, updated_props));
            annotated_count += 1;
        }
    }

    // Batch update all nodes in a single transaction
    if !updates.is_empty() {
        if let Err(e) = store.update_node_properties_batch(&updates) {
            tracing::warn!(
                error = %e,
                "pass_decorators: failed to batch update node properties"
            );
        }
    }

    tracing::info!(
        annotated = annotated_count,
        files = file_decorators.len(),
        "pass_decorators: completed decorator extraction"
    );
}

// ── Types ─────────────────────────────────────────────────────────────────

/// A decorator occurrence found in source code.
#[derive(Debug, Clone)]
struct DecoratorOccurrence {
    /// The normalized decorator name (unqualified, no @, no arguments).
    name: String,
    /// The 1-based line number where the decorator appears.
    line: i32,
}

// ── Helper functions ──────────────────────────────────────────────────────

/// Check if a language supports decorator/annotation extraction.
fn is_decorator_language(lang: Language) -> bool {
    matches!(
        lang,
        Language::Python | Language::Java | Language::Kotlin | Language::TypeScript | Language::Tsx
    )
}

/// Extract all decorator/annotation occurrences from source code.
/// Returns a list of (normalized_name, line_number) pairs in declaration order.
fn extract_decorators(source: &str, lang: Language) -> Vec<DecoratorOccurrence> {
    let re = match lang {
        Language::Python => &*PYTHON_DECORATOR_RE,
        Language::Java | Language::Kotlin => &*JAVA_ANNOTATION_RE,
        Language::TypeScript | Language::Tsx => &*TS_DECORATOR_RE,
        _ => return Vec::new(),
    };

    let mut decorators = Vec::new();

    for cap in re.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            let full_name = m.as_str();
            let normalized = normalize_decorator_name(full_name);

            // Skip empty names or Java/Kotlin built-in annotations that are noise
            if normalized.is_empty() || is_noise_annotation(&normalized, lang) {
                continue;
            }

            // Compute line number from byte offset
            let line = byte_offset_to_line(source, m.start());

            decorators.push(DecoratorOccurrence {
                name: normalized,
                line,
            });
        }
    }

    decorators
}

/// Normalize a decorator name:
/// - Strip the leading `@` (already done by regex capture)
/// - Strip arguments (already done by regex not capturing parens)
/// - Store only the unqualified name (last segment after `.`)
///
/// Examples:
///   "app.route" -> "route"
///   "RequestMapping" -> "RequestMapping"
///   "flask.ext.login.login_required" -> "login_required"
pub fn normalize_decorator_name(full_name: &str) -> String {
    // Take the last segment after the last dot (unqualified name)
    full_name.rsplit('.').next().unwrap_or(full_name).to_owned()
}

/// Normalize a list of raw decorator strings.
///
/// Each raw decorator may include `@` prefix, qualified paths, and arguments.
/// This function normalizes each one: strips @, strips arguments (from `(`),
/// takes unqualified name (last segment after `.`).
/// Preserves declaration order and caps at MAX_DECORATORS_PER_NODE.
///
/// This is the public entry point for normalizing decorator lists as stored
/// by tree-sitter walkers (which store raw text like `@Component({...})`).
pub fn normalize_decorator_list(raw_decorators: &[String]) -> Vec<String> {
    raw_decorators
        .iter()
        .map(|raw| {
            // Strip leading @
            let s = raw.strip_prefix('@').unwrap_or(raw);
            // Strip arguments (from first `(` onward)
            let s = s.split('(').next().unwrap_or(s).trim();
            // Take unqualified name (last segment after `.`)
            normalize_decorator_name(s)
        })
        .filter(|s| !s.is_empty())
        .take(MAX_DECORATORS_PER_NODE)
        .collect()
}

/// Check if an annotation is a noise annotation that shouldn't be stored.
/// These are very common Java/Kotlin annotations that add little value for querying.
/// Note: We keep this minimal - only filtering @Override and @SuppressWarnings
/// which are compiler directives rather than meaningful annotations.
fn is_noise_annotation(name: &str, lang: Language) -> bool {
    match lang {
        Language::Java | Language::Kotlin => {
            matches!(name, "Override" | "SuppressWarnings")
        }
        _ => false,
    }
}

/// Convert a byte offset to a 1-based line number.
fn byte_offset_to_line(source: &str, offset: usize) -> i32 {
    source[..offset].bytes().filter(|&b| b == b'\n').count() as i32 + 1
}

/// Associate decorators with nodes based on line proximity.
///
/// A decorator is associated with the node whose `start_line` is the closest
/// line at or after the decorator's line. This handles the common pattern where
/// decorators appear on lines immediately before the decorated symbol.
///
/// Returns a map of node_id -> list of decorator names (in declaration order).
fn associate_decorators_with_nodes(
    nodes: &[codryn_store::Node],
    decorators: &[DecoratorOccurrence],
) -> HashMap<i64, Vec<String>> {
    let mut result: HashMap<i64, Vec<String>> = HashMap::new();

    if decorators.is_empty() {
        return result;
    }

    // Sort nodes by start_line for efficient lookup
    let mut sorted_nodes: Vec<&codryn_store::Node> = nodes.iter().collect();
    sorted_nodes.sort_by_key(|n| n.start_line);

    // For each decorator, find the node it decorates
    // A decorator on line L decorates the next node that starts on line >= L
    for dec in decorators {
        // Find the first node whose start_line >= decorator line
        // The decorator must be within a reasonable distance (e.g., within 10 lines)
        let candidate = sorted_nodes
            .iter()
            .find(|n| n.start_line >= dec.line && n.start_line - dec.line <= 10);

        if let Some(node) = candidate {
            let entry = result.entry(node.id).or_default();
            // Enforce max 50 decorators per node (Requirement 22.6)
            if entry.len() < MAX_DECORATORS_PER_NODE {
                entry.push(dec.name.clone());
            }
        }
    }

    result
}

/// Set the `decorators` property on a node's properties JSON.
///
/// If the node already has properties, merges the `decorators` field into the
/// existing JSON object. If properties are empty/null, creates a new JSON object.
/// Stores an empty list for nodes without decorators (Requirement 22.5).
fn set_decorators_property(existing_props: Option<&str>, decorators: &[String]) -> String {
    let decorators_value = serde_json::Value::Array(
        decorators
            .iter()
            .map(|d| serde_json::Value::String(d.clone()))
            .collect(),
    );

    let decorators_json = serde_json::to_string(&decorators_value).unwrap();

    match existing_props {
        Some(json_str) if !json_str.is_empty() => {
            match serde_json::from_str::<serde_json::Value>(json_str) {
                Ok(mut obj) => {
                    if let Some(map) = obj.as_object_mut() {
                        map.insert("decorators".to_string(), decorators_value);
                    }
                    serde_json::to_string(&obj)
                        .unwrap_or_else(|_| format!(r#"{{"decorators":{}}}"#, decorators_json))
                }
                Err(_) => {
                    // If existing properties aren't valid JSON, create fresh
                    format!(r#"{{"decorators":{}}}"#, decorators_json)
                }
            }
        }
        _ => {
            // No existing properties — create new JSON object
            format!(r#"{{"decorators":{}}}"#, decorators_json)
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_decorator_name_simple() {
        assert_eq!(normalize_decorator_name("Component"), "Component");
        assert_eq!(normalize_decorator_name("Injectable"), "Injectable");
    }

    #[test]
    fn test_normalize_decorator_name_dotted() {
        assert_eq!(normalize_decorator_name("app.route"), "route");
        assert_eq!(
            normalize_decorator_name("flask.ext.login.login_required"),
            "login_required"
        );
        assert_eq!(
            normalize_decorator_name("org.springframework.web.bind.annotation.RequestMapping"),
            "RequestMapping"
        );
    }

    #[test]
    fn test_extract_decorators_python() {
        let source = r#"
@app.route("/api/users")
@login_required
def get_users():
    pass

@dataclass
class User:
    name: str
"#;
        let decorators = extract_decorators(source, Language::Python);
        let names: Vec<&str> = decorators.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["route", "login_required", "dataclass"]);
    }

    #[test]
    fn test_extract_decorators_java() {
        let source = r#"
@RestController
@RequestMapping("/api")
public class UserController {

    @GetMapping("/users")
    @Transactional
    public List<User> getUsers() {
        return userService.findAll();
    }
}
"#;
        let decorators = extract_decorators(source, Language::Java);
        let names: Vec<&str> = decorators.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"RestController"));
        assert!(names.contains(&"RequestMapping"));
        assert!(names.contains(&"GetMapping"));
        assert!(names.contains(&"Transactional"));
        // Override should be filtered as noise
        assert!(!names.contains(&"Override"));
    }

    #[test]
    fn test_extract_decorators_kotlin() {
        let source = r#"
@Service
class UserService {

    @Transactional
    fun createUser(dto: CreateUserDto): User {
        return userRepository.save(dto.toEntity())
    }
}
"#;
        let decorators = extract_decorators(source, Language::Kotlin);
        let names: Vec<&str> = decorators.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Service"));
        assert!(names.contains(&"Transactional"));
    }

    #[test]
    fn test_extract_decorators_typescript() {
        let source = r#"
@Component({
    selector: 'app-root',
    templateUrl: './app.component.html'
})
@Injectable({ providedIn: 'root' })
export class AppComponent {

    @Input()
    title: string;

    @HostListener('click')
    onClick() {}
}
"#;
        let decorators = extract_decorators(source, Language::TypeScript);
        let names: Vec<&str> = decorators.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Component"));
        assert!(names.contains(&"Injectable"));
        assert!(names.contains(&"Input"));
        assert!(names.contains(&"HostListener"));
    }

    #[test]
    fn test_extract_decorators_java_with_override_filtered() {
        let source = r#"
@Override
public String toString() {
    return "test";
}

@Deprecated
@SuppressWarnings("unchecked")
public void oldMethod() {}
"#;
        let decorators = extract_decorators(source, Language::Java);
        let names: Vec<&str> = decorators.iter().map(|d| d.name.as_str()).collect();
        // Override and SuppressWarnings are noise (compiler directives)
        assert!(!names.contains(&"Override"));
        assert!(!names.contains(&"SuppressWarnings"));
        // Deprecated is a meaningful annotation and should be kept
        assert!(names.contains(&"Deprecated"));
    }

    #[test]
    fn test_extract_decorators_empty_file() {
        let source = "def hello():\n    pass\n";
        let decorators = extract_decorators(source, Language::Python);
        assert!(decorators.is_empty());
    }

    #[test]
    fn test_extract_decorators_unsupported_language() {
        let source = "fn main() {}";
        let decorators = extract_decorators(source, Language::Rust);
        assert!(decorators.is_empty());
    }

    #[test]
    fn test_byte_offset_to_line() {
        let source = "line1\nline2\nline3";
        assert_eq!(byte_offset_to_line(source, 0), 1); // start of line 1
        assert_eq!(byte_offset_to_line(source, 5), 1); // newline char
        assert_eq!(byte_offset_to_line(source, 6), 2); // start of line 2
        assert_eq!(byte_offset_to_line(source, 12), 3); // start of line 3
    }

    #[test]
    fn test_set_decorators_property_empty() {
        let result = set_decorators_property(None, &[]);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["decorators"], serde_json::json!([]));
    }

    #[test]
    fn test_set_decorators_property_with_decorators() {
        let decorators = vec!["Component".to_string(), "Injectable".to_string()];
        let result = set_decorators_property(None, &decorators);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["decorators"],
            serde_json::json!(["Component", "Injectable"])
        );
    }

    #[test]
    fn test_set_decorators_property_merge_existing() {
        let existing = r#"{"language":"typescript","complexity":3}"#;
        let decorators = vec!["Service".to_string()];
        let result = set_decorators_property(Some(existing), &decorators);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["decorators"], serde_json::json!(["Service"]));
        assert_eq!(parsed["language"], "typescript");
        assert_eq!(parsed["complexity"], 3);
    }

    #[test]
    fn test_set_decorators_property_invalid_json() {
        let decorators = vec!["Route".to_string()];
        let result = set_decorators_property(Some("not valid json"), &decorators);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["decorators"], serde_json::json!(["Route"]));
    }

    #[test]
    fn test_associate_decorators_with_nodes() {
        let nodes = vec![
            codryn_store::Node {
                id: 1,
                project: "test".to_string(),
                label: "Class".to_string(),
                name: "UserController".to_string(),
                qualified_name: "test.UserController".to_string(),
                file_path: "src/controller.java".to_string(),
                start_line: 3,
                end_line: 20,
                properties_json: None,
            },
            codryn_store::Node {
                id: 2,
                project: "test".to_string(),
                label: "Method".to_string(),
                name: "getUsers".to_string(),
                qualified_name: "test.UserController.getUsers".to_string(),
                file_path: "src/controller.java".to_string(),
                start_line: 6,
                end_line: 10,
                properties_json: None,
            },
        ];

        let decorators = vec![
            DecoratorOccurrence {
                name: "RestController".to_string(),
                line: 1,
            },
            DecoratorOccurrence {
                name: "RequestMapping".to_string(),
                line: 2,
            },
            DecoratorOccurrence {
                name: "GetMapping".to_string(),
                line: 5,
            },
        ];

        let result = associate_decorators_with_nodes(&nodes, &decorators);

        // RestController and RequestMapping should be on the class (start_line=3)
        let class_decs = result.get(&1).unwrap();
        assert_eq!(class_decs, &vec!["RestController", "RequestMapping"]);

        // GetMapping should be on the method (start_line=6)
        let method_decs = result.get(&2).unwrap();
        assert_eq!(method_decs, &vec!["GetMapping"]);
    }

    #[test]
    fn test_associate_decorators_preserves_order() {
        let nodes = vec![codryn_store::Node {
            id: 1,
            project: "test".to_string(),
            label: "Function".to_string(),
            name: "handler".to_string(),
            qualified_name: "test.handler".to_string(),
            file_path: "src/app.py".to_string(),
            start_line: 4,
            end_line: 10,
            properties_json: None,
        }];

        let decorators = vec![
            DecoratorOccurrence {
                name: "route".to_string(),
                line: 1,
            },
            DecoratorOccurrence {
                name: "login_required".to_string(),
                line: 2,
            },
            DecoratorOccurrence {
                name: "cache".to_string(),
                line: 3,
            },
        ];

        let result = associate_decorators_with_nodes(&nodes, &decorators);
        let decs = result.get(&1).unwrap();
        // Order should be preserved as declared
        assert_eq!(decs, &vec!["route", "login_required", "cache"]);
    }

    #[test]
    fn test_max_decorators_limit() {
        let nodes = vec![codryn_store::Node {
            id: 1,
            project: "test".to_string(),
            label: "Class".to_string(),
            name: "BigClass".to_string(),
            qualified_name: "test.BigClass".to_string(),
            file_path: "src/big.py".to_string(),
            start_line: 60,
            end_line: 100,
            properties_json: None,
        }];

        // Create 55 decorators all within 10 lines of start_line=60
        let decorators: Vec<DecoratorOccurrence> = (0..55)
            .map(|i| DecoratorOccurrence {
                name: format!("decorator_{}", i),
                line: 50 + (i % 10), // lines 50-59, all within 10 lines of start_line=60
            })
            .collect();

        let result = associate_decorators_with_nodes(&nodes, &decorators);
        let decs = result.get(&1).unwrap();
        assert_eq!(decs.len(), MAX_DECORATORS_PER_NODE);
        // First 50 should be preserved in order
        assert_eq!(decs[0], "decorator_0");
        assert_eq!(decs[49], "decorator_49");
    }

    #[test]
    fn test_is_decorator_language() {
        assert!(is_decorator_language(Language::Python));
        assert!(is_decorator_language(Language::Java));
        assert!(is_decorator_language(Language::Kotlin));
        assert!(is_decorator_language(Language::TypeScript));
        assert!(is_decorator_language(Language::Tsx));
        assert!(!is_decorator_language(Language::Rust));
        assert!(!is_decorator_language(Language::Go));
        assert!(!is_decorator_language(Language::C));
    }
}
