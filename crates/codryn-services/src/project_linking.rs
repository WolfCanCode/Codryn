use anyhow::Result;
use codryn_store::Store;
use serde::Serialize;

pub struct ProjectLinkingService;

#[derive(Debug, Serialize)]
pub struct LinkSuggestionResult {
    pub suggestions: Vec<LinkSuggestion>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct LinkSuggestion {
    pub project: String,
    pub target_project: String,
    pub reason: String,
    pub score: f64,
}

impl ProjectLinkingService {
    pub fn suggest_links(
        store: &Store,
        project: Option<&str>,
        limit: i32,
    ) -> Result<LinkSuggestionResult> {
        let limit = if limit <= 0 { 10usize } else { limit as usize };
        let projects = store.list_projects()?;
        if projects.len() < 2 {
            return Ok(LinkSuggestionResult {
                suggestions: vec![],
                count: 0,
            });
        }

        let existing_links: std::collections::HashSet<(String, String)> = projects
            .iter()
            .flat_map(|p| {
                store
                    .get_linked_projects(&p.name)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|l| (l.source_project, l.target_project))
            })
            .collect();

        let mut suggestions: Vec<LinkSuggestion> = Vec::new();

        let candidates: Vec<&codryn_store::Project> = if let Some(p) = project {
            projects.iter().filter(|pr| pr.name != p).collect()
        } else {
            projects.iter().collect()
        };

        // Compare all pairs
        for a in &projects {
            if let Some(p) = project {
                if a.name != p {
                    continue;
                }
            }
            for b in &candidates {
                if a.name >= b.name {
                    continue;
                } // avoid duplicates
                if existing_links.contains(&(a.name.clone(), b.name.clone())) {
                    continue;
                }

                let mut score: i32 = 0;
                let mut reasons: Vec<&str> = Vec::new();

                // 1. Name similarity
                if names_related(&a.name, &b.name) {
                    score += 25;
                    reasons.push("related project names");
                }

                // 2. Shared symbol names (DTO/type overlap)
                let shared = store
                    .find_matching_symbols_across_projects(&a.name, &b.name)
                    .unwrap_or_default();
                if !shared.is_empty() {
                    score += (shared.len() as i32 * 10).min(30);
                    reasons.push("shared type/DTO names");
                }

                // 3. Domain keyword overlap
                let a_keywords = extract_domain_keywords(&a.name);
                let b_keywords = extract_domain_keywords(&b.name);
                let overlap: usize = a_keywords.iter().filter(|k| b_keywords.contains(k)).count();
                if overlap > 0 {
                    score += (overlap as i32 * 8).min(15);
                    if !reasons.contains(&"related project names") {
                        reasons.push("domain keyword overlap");
                    }
                }

                if score > 0 {
                    suggestions.push(LinkSuggestion {
                        project: a.name.clone(),
                        target_project: b.name.clone(),
                        reason: reasons.join(", "),
                        score: (score as f64 / 100.0).clamp(0.0, 1.0),
                    });
                }
            }
        }

        suggestions.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        suggestions.truncate(limit);
        let count = suggestions.len();
        Ok(LinkSuggestionResult { suggestions, count })
    }
}

fn names_related(a: &str, b: &str) -> bool {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    // One contains the other, or they share a significant prefix
    if a_lower.contains(&b_lower) || b_lower.contains(&a_lower) {
        return true;
    }
    // Common suffixes like -engine, -api, -frontend, -backend, -service
    let strip = |s: &str| -> String {
        s.replace("-engine", "")
            .replace("-api", "")
            .replace("-frontend", "")
            .replace("-backend", "")
            .replace("-service", "")
            .replace("-web", "")
            .replace("-app", "")
            .replace("-core", "")
    };
    strip(&a_lower) == strip(&b_lower)
}

fn extract_domain_keywords(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(['-', '_', '.'])
        .filter(|s| s.len() > 2)
        .filter(|s| {
            !matches!(
                *s,
                "api"
                    | "app"
                    | "web"
                    | "service"
                    | "engine"
                    | "core"
                    | "frontend"
                    | "backend"
                    | "src"
                    | "main"
            )
        })
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codryn_store::{Node, Project};

    #[test]
    fn test_suggest_links_related_names() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "travelmate".into(),
            indexed_at: "now".into(),
            root_path: "/a".into(),
        })
        .unwrap();
        s.upsert_project(&Project {
            name: "travelmate-engine".into(),
            indexed_at: "now".into(),
            root_path: "/b".into(),
        })
        .unwrap();

        let res = ProjectLinkingService::suggest_links(&s, None, 10).unwrap();
        assert!(!res.suggestions.is_empty());
        assert!(res.suggestions[0].reason.contains("related project names"));
    }

    #[test]
    fn test_suggest_links_shared_types() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "fe".into(),
            indexed_at: "now".into(),
            root_path: "/fe".into(),
        })
        .unwrap();
        s.upsert_project(&Project {
            name: "be".into(),
            indexed_at: "now".into(),
            root_path: "/be".into(),
        })
        .unwrap();
        // Same class name in both projects
        s.insert_node(&Node {
            id: 0,
            project: "fe".into(),
            label: "Class".into(),
            name: "UserDto".into(),
            qualified_name: "fe::UserDto".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();
        s.insert_node(&Node {
            id: 0,
            project: "be".into(),
            label: "Class".into(),
            name: "UserDto".into(),
            qualified_name: "be::UserDto".into(),
            file_path: "".into(),
            start_line: 0,
            end_line: 0,
            properties_json: None,
        })
        .unwrap();

        let res = ProjectLinkingService::suggest_links(&s, None, 10).unwrap();
        assert!(!res.suggestions.is_empty());
        assert!(res.suggestions[0].reason.contains("shared type"));
    }

    #[test]
    fn test_no_suggestions_for_single_project() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_project(&Project {
            name: "solo".into(),
            indexed_at: "now".into(),
            root_path: "/".into(),
        })
        .unwrap();
        let res = ProjectLinkingService::suggest_links(&s, None, 10).unwrap();
        assert_eq!(res.count, 0);
    }
}
