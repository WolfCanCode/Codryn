use std::collections::HashMap;

/// Intern pool — deduplicates strings, returning stable `&str` references.
/// Backed by a HashMap for O(1) lookup.
pub struct StringInterner {
    map: HashMap<String, ()>,
}

impl StringInterner {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    /// Intern a string, returning a reference valid for the interner's lifetime.
    pub fn intern(&mut self, s: &str) -> &str {
        if !self.map.contains_key(s) {
            self.map.insert(s.to_owned(), ());
        }
        // SAFETY: key lives as long as the map
        unsafe {
            let key = self.map.get_key_value(s).unwrap().0;
            &*(key.as_str() as *const str)
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Default for StringInterner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_dedup() {
        let mut pool = StringInterner::new();
        let a = pool.intern("hello") as *const str;
        let b = pool.intern("hello") as *const str;
        assert!(std::ptr::eq(a, b));
        assert_eq!(pool.len(), 1);
    }
}
