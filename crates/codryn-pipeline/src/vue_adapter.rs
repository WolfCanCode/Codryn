use codryn_discover::{DiscoveredFile, Language};
use codryn_foundation::fqn;
use codryn_graph_buffer::GraphBuffer;
use codryn_store::Store;
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

static VUE_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"name\s*:\s*['"](\w+)['"]"#).unwrap());
static VUE_ELEM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<([\w]+-[\w-]+)"#).unwrap());
static VUE_COMPOSABLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"import\s+(\w+)\s+from\s+['"]@/composables/"#).unwrap());

/// Convert PascalCase to kebab-case.
fn to_kebab(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Derive a selector from a `.vue` filename.
fn selector_from_filename(rel_path: &str) -> Option<String> {
    let name = rel_path.rsplit('/').next()?;
    let stem = name.strip_suffix(".vue")?;
    Some(to_kebab(stem))
}

/// Pass: Extract Vue component Selector nodes, SELECTS, INJECTS, and RENDERS edges.
pub fn pass_vue(buf: &mut GraphBuffer, store: &Store, files: &[&DiscoveredFile], project: &str) {
    for f in files {
        if f.language != Language::Vue {
            continue;
        }
        let source = match std::fs::read_to_string(&f.abs_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let comp_name = VUE_NAME_RE
            .captures(&source)
            .map(|c| c.get(1).unwrap().as_str().to_string())
            .or_else(|| {
                let name = f.rel_path.rsplit('/').next()?;
                Some(name.strip_suffix(".vue")?.to_string())
            });

        let comp_name = match comp_name {
            Some(n) => n,
            None => continue,
        };

        let selector = VUE_NAME_RE
            .captures(&source)
            .map(|c| to_kebab(c.get(1).unwrap().as_str()))
            .or_else(|| selector_from_filename(&f.rel_path));

        let class_qn = fqn::fqn_compute(project, &f.rel_path, Some(&comp_name));

        if let Some(ref sel) = selector {
            let sq = format!("{project}.selector.{sel}");
            let props = serde_json::json!({"component": comp_name, "selector": sel}).to_string();
            buf.add_node("Selector", sel, &sq, &f.rel_path, 0, 0, Some(props));
            buf.add_edge_by_qn(&sq, &class_qn, "SELECTS", None);
        }

        for cap in VUE_COMPOSABLE_RE.captures_iter(&source) {
            let composable = cap.get(1).unwrap().as_str();
            buf.add_edge_by_qn(
                &class_qn,
                &format!("{project}.{composable}"),
                "INJECTS",
                None,
            );
        }

        let mut seen = HashSet::new();
        for cap in VUE_ELEM_RE.captures_iter(&source) {
            let sel = cap.get(1).unwrap().as_str();
            if !seen.insert(sel.to_owned()) {
                continue;
            }
            if let Ok(Some(child)) = store.find_node_by_property(project, "selector", sel) {
                buf.add_edge_by_qn(&class_qn, &child.qualified_name, "RENDERS", None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_conversion() {
        assert_eq!(to_kebab("Dropdown"), "dropdown");
        assert_eq!(to_kebab("ModelSelect"), concat!("model", "-", "select"));
        assert_eq!(
            to_kebab("GptResourceChunksModal"),
            ["gpt", "resource", "chunks", "modal"].join("-")
        );
        assert_eq!(to_kebab("DropdownBase"), ["dropdown", "base"].join("-"));
    }

    #[test]
    fn selector_from_file() {
        let result = selector_from_filename("src/components/ModelSelect.vue");
        assert_eq!(result, Some(["model", "select"].join("-")));
    }

    #[test]
    fn name_regex() {
        let src = r#"export default { name: "DropdownBase", setup() {} }"#;
        let cap = VUE_NAME_RE.captures(src).unwrap();
        assert_eq!(cap.get(1).unwrap().as_str(), "DropdownBase");
    }

    #[test]
    fn composable_regex() {
        let src = r#"import useDropdown from "@/composables/useDropdown";"#;
        let cap = VUE_COMPOSABLE_RE.captures(src).unwrap();
        assert_eq!(cap.get(1).unwrap().as_str(), "useDropdown");
    }
}
