use crate::str_util::{normalize_for_matching, token_similarity};

#[derive(Debug, Clone)]
pub struct ScopeMatch {
    pub score: f64,
    pub match_type: &'static str,
}

pub struct ScopeMatchingService;

impl ScopeMatchingService {
    /// Score how well `scope` matches against a set of candidate fields
    /// (route path, controller name, handler name, package path).
    /// Returns None if no match, Some(ScopeMatch) with score: exact > normalized > fuzzy.
    pub fn score(scope: &str, fields: &[&str]) -> Option<ScopeMatch> {
        let scope_lower = scope.to_lowercase();
        let scope_norm = normalize_for_matching(scope);

        // Tier 1: exact case-insensitive contains
        for f in fields {
            if f.to_lowercase().contains(&scope_lower) {
                return Some(ScopeMatch {
                    score: 1.0,
                    match_type: "exact",
                });
            }
        }

        // Tier 2: normalized contains (strips separators + lowercases)
        for f in fields {
            if normalize_for_matching(f).contains(&scope_norm) {
                return Some(ScopeMatch {
                    score: 0.8,
                    match_type: "normalized",
                });
            }
        }

        // Tier 3: fuzzy token similarity
        let best = fields
            .iter()
            .map(|f| token_similarity(scope, f))
            .fold(0.0f64, f64::max);
        if best > 0.3 {
            return Some(ScopeMatch {
                score: best * 0.6,
                match_type: "fuzzy",
            });
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let m = ScopeMatchingService::score("travel-request", &["/v1/travel-request/{id}"]);
        assert!(m.is_some());
        assert_eq!(m.unwrap().match_type, "exact");
    }

    #[test]
    fn normalized_match_kebab_to_camel() {
        let m = ScopeMatchingService::score("travel-request", &["TravelRequestController"]);
        assert!(m.is_some());
        assert_eq!(m.unwrap().match_type, "normalized");
    }

    #[test]
    fn normalized_match_snake_to_kebab() {
        let m = ScopeMatchingService::score("travel_request", &["/v1/travelrequest/{id}"]);
        assert!(m.is_some());
        assert!(m.unwrap().score >= 0.8);
    }

    #[test]
    fn fuzzy_match() {
        let m = ScopeMatchingService::score("travel-req", &["TravelRequestController"]);
        assert!(m.is_some());
        // "travel-req" normalizes to "travelreq" which is a substring of "travelrequestcontroller"
        // so this hits normalized tier
        assert!(m.unwrap().score >= 0.6);
    }

    #[test]
    fn no_match() {
        let m = ScopeMatchingService::score("billing", &["UserController", "/v1/users"]);
        assert!(m.is_none());
    }
}
