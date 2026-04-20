use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::Store;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static INJECT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:private|protected|public|readonly)\s+\w+\s*:\s*(\w+)").unwrap()
});
static ELEM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<([\w]+-[\w-]+)").unwrap());

const DECORATORS: &[(&str, &str)] = &[
    ("Component", "component"),
    ("Injectable", "service"),
    ("Directive", "directive"),
    ("Pipe", "pipe"),
    ("NgModule", "module"),
    ("Guard", "service"),
    ("Resolver", "service"),
    ("Interceptor", "service"),
];

const SKIP_INJECT: &[&str] = &[
    "string",
    "number",
    "boolean",
    "any",
    "String",
    "Number",
    "Boolean",
    "Object",
    "void",
    "EventEmitter",
    "ElementRef",
    "ChangeDetectorRef",
    "Renderer2",
    "Injector",
    "TemplateRef",
    "ViewContainerRef",
    "NgZone",
];

pub fn pass_angular(
    buf: &mut GraphBuffer,
    store: &Store,
    files: &[&DiscoveredFile],
    project: &str,
) {
    for f in files {
        if !matches!(f.language, Language::TypeScript | Language::Tsx) {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut in_ctor = false;
        let mut class_qn: Option<String> = None;
        let mut pending_sel: Option<String> = None;
        let mut pending_dec: Option<String> = None;

        for line in source.lines() {
            let t = line.trim();
            for (dec, _) in DECORATORS {
                if t.contains(&format!("@{dec}")) {
                    pending_dec = Some(dec.to_string());
                    if *dec == "Component" {
                        pending_sel = find_selector(&source, t);
                    }
                    break;
                }
            }
            if t.contains("class ") && pending_dec.is_some() {
                if let Some(name) = class_name(t) {
                    let qn = fqn::fqn_compute(project, &f.rel_path, Some(&name));
                    class_qn = Some(qn.clone());
                    if let Some(sel) = pending_sel.take() {
                        let sq = format!("{project}.selector.{sel}");
                        let p = serde_json::json!({"component": name, "selector": sel}).to_string();
                        buf.add_node("Selector", &sel, &sq, &f.rel_path, 0, 0, Some(p));
                        buf.add_edge_by_qn(&sq, &qn, "SELECTS", None);
                    }
                    pending_dec = None;
                }
            }
            if let Some(ref cqn) = class_qn {
                if t.contains("constructor(") || t.contains("constructor (") {
                    in_ctor = true;
                }
                if in_ctor {
                    for c in INJECT_RE.captures_iter(t) {
                        let svc = c.get(1).unwrap().as_str();
                        if !SKIP_INJECT.contains(&svc) {
                            buf.add_edge_by_qn(cqn, &format!("{project}.{svc}"), "INJECTS", None);
                        }
                    }
                    if t.contains(')') {
                        in_ctor = false;
                    }
                }
            }
        }
        // Inline template
        let cqn = store
            .get_nodes_for_file(project, &f.rel_path)
            .ok()
            .and_then(|ns| {
                ns.into_iter()
                    .find(|n| n.label == "Class")
                    .map(|n| n.qualified_name)
            });
        if let Some(ref cqn) = cqn {
            if let Some(tpl) = inline_tpl(&source) {
                let mut seen = HashSet::new();
                for c in ELEM_RE.captures_iter(&tpl) {
                    let sel = c.get(1).unwrap().as_str();
                    if seen.insert(sel.to_owned()) {
                        if let Ok(Some(child)) =
                            store.find_node_by_property(project, "selector", sel)
                        {
                            buf.add_edge_by_qn(cqn, &child.qualified_name, "RENDERS", None);
                        }
                    }
                }
            }
        }
    }
}

pub fn pass_angular_classify(store: &Store, project: &str) {
    for node in store.get_all_nodes(project).unwrap_or_default() {
        if node.label != "Class" {
            continue;
        }
        let ps = node.properties_json.as_deref().unwrap_or("{}");
        let mut v: serde_json::Value = serde_json::from_str(ps).unwrap_or_default();
        if v.get("layer").is_some() {
            continue;
        }
        let dec = v.get("decorator").and_then(|d| d.as_str()).unwrap_or("");
        if let Some((_, layer)) = DECORATORS.iter().find(|(d, _)| *d == dec) {
            v["layer"] = serde_json::json!(layer);
            let _ = store.update_node_properties(node.id, &v.to_string());
        }
    }
}

fn find_selector(source: &str, dec_line: &str) -> Option<String> {
    if let Some(s) = sel_val(dec_line) {
        return Some(s);
    }
    let i = source.find(dec_line)?;
    let block = &source[i..];
    sel_val(&block[..block.find(')')? + 1])
}

fn sel_val(t: &str) -> Option<String> {
    let i = t.find("selector")?;
    let r = &t[i..];
    let q = r.find(['\'', '"', '`'])?;
    let ch = r.as_bytes()[q] as char;
    let a = &r[q + 1..];
    Some(a[..a.find(ch)?].to_string())
}

fn class_name(line: &str) -> Option<String> {
    let i = line.find("class ")?;
    let r = &line[i + 6..];
    let n: String = r
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if n.is_empty() {
        None
    } else {
        Some(n)
    }
}

fn inline_tpl(source: &str) -> Option<String> {
    let i = source.find("template:")?;
    let r = &source[i..];
    let q = r.find(['`', '\''])?;
    let ch = r.as_bytes()[q] as char;
    let a = &r[q + 1..];
    Some(a[..a.find(ch)?].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selector_extraction() {
        assert_eq!(
            sel_val("selector: 'app-user-card', templateUrl: './u.html'"),
            Some("app-user-card".into())
        );
        assert_eq!(
            sel_val(r#"selector: "request-details-drawer""#),
            Some("request-details-drawer".into())
        );
    }

    #[test]
    fn test_class_name_parse() {
        assert_eq!(
            class_name("export class UserCardComponent implements OnInit {"),
            Some("UserCardComponent".into())
        );
    }

    #[test]
    fn test_inline_template_parse() {
        let s = "@Component({ selector: 'app-root', template: `<app-header></app-header>` })";
        assert!(inline_tpl(s).unwrap().contains("app-header"));
    }

    #[test]
    fn test_di_regex() {
        let line =
            "constructor(private userService: UserService, readonly authService: AuthService) {";
        let m: Vec<String> = INJECT_RE
            .captures_iter(line)
            .map(|c| c.get(1).unwrap().as_str().into())
            .collect();
        assert_eq!(m, vec!["UserService", "AuthService"]);
    }

    #[test]
    fn test_selector_node_and_di() {
        use codryn_store::{Node, Project, Store};
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();
        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "UserCardComponent".into(),
                qualified_name: "p.src.app.user-card.component.UserCardComponent".into(),
                file_path: "src/app/user-card.component.ts".into(),
                start_line: 5,
                end_line: 50,
                properties_json: Some(
                    r#"{"decorator":"Component","selector":"app-user-card"}"#.into(),
                ),
            })
            .unwrap();
        let dir = std::env::temp_dir().join("codryn_ng_test");
        let _ = std::fs::create_dir_all(&dir);
        let tp = dir.join("user-card.component.ts");
        std::fs::write(&tp, "@Component({ selector: 'app-user-card' })\nexport class UserCardComponent {\n  constructor(private userService: UserService) {}\n}\n").unwrap();
        let file = DiscoveredFile {
            abs_path: tp,
            rel_path: "src/app/user-card.component.ts".into(),
            language: Language::TypeScript,
        };
        let mut buf = GraphBuffer::new("p");
        pass_angular(&mut buf, &store, &[&file], "p");
        assert!(
            buf.node_count() >= 1,
            "selector node missing, got {}",
            buf.node_count()
        );
        assert!(
            buf.edge_count() >= 2,
            "SELECTS+INJECTS missing, got {}",
            buf.edge_count()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_angular_classify() {
        use codryn_store::{Node, Project, Store};
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/".into(),
            })
            .unwrap();
        store
            .insert_node(&Node {
                id: 0,
                project: "p".into(),
                label: "Class".into(),
                name: "FooService".into(),
                qualified_name: "p.FooService".into(),
                file_path: "src/foo.service.ts".into(),
                start_line: 1,
                end_line: 10,
                properties_json: Some(r#"{"decorator":"Injectable"}"#.into()),
            })
            .unwrap();
        pass_angular_classify(&store, "p");
        let n = store.find_node_by_qn("p", "p.FooService").unwrap().unwrap();
        let props: serde_json::Value =
            serde_json::from_str(n.properties_json.as_deref().unwrap()).unwrap();
        assert_eq!(props["layer"], "service");
    }

    #[test]
    fn test_no_duplicate_selectors() {
        // Ensure non-Angular TS files don't produce selector nodes
        use codryn_store::{Project, Store};
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_project(&Project {
                name: "p".into(),
                indexed_at: "now".into(),
                root_path: "/tmp".into(),
            })
            .unwrap();
        let dir = std::env::temp_dir().join("codryn_ng_nodup");
        let _ = std::fs::create_dir_all(&dir);
        let tp = dir.join("plain.ts");
        std::fs::write(
            &tp,
            "export class PlainClass {\n  constructor(private x: string) {}\n}\n",
        )
        .unwrap();
        let file = DiscoveredFile {
            abs_path: tp,
            rel_path: "src/plain.ts".into(),
            language: Language::TypeScript,
        };
        let mut buf = GraphBuffer::new("p");
        pass_angular(&mut buf, &store, &[&file], "p");
        assert_eq!(
            buf.node_count(),
            0,
            "plain TS should produce no selector nodes"
        );
        assert_eq!(
            buf.edge_count(),
            0,
            "plain TS should produce no Angular edges"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
