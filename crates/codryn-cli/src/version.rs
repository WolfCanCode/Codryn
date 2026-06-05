/// Compare two semver strings. Returns >0 if a > b, <0 if a < b, 0 if equal.
pub fn compare_versions(a: &str, b: &str) -> i32 {
    let parse = |s: &str| -> (u32, u32, u32) {
        let s = s.trim_start_matches('v').split('-').next().unwrap_or(s);
        let parts: Vec<u32> = s.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    let (a1, a2, a3) = parse(a);
    let (b1, b2, b3) = parse(b);
    if a1 != b1 {
        return (a1 as i32) - (b1 as i32);
    }
    if a2 != b2 {
        return (a2 as i32) - (b2 as i32);
    }
    (a3 as i32) - (b3 as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare() {
        assert!(compare_versions("0.2.1", "0.2.0") > 0);
        assert!(compare_versions("0.1.0", "0.2.0") < 0);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), 0);
        assert!(compare_versions("v1.0.0", "0.9.9") > 0);
    }
}
