use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Number of hash permutations used for MinHash fingerprinting.
pub const NUM_HASHES: usize = 128;

/// A MinHash fingerprint — a fixed-size signature for similarity comparison.
///
/// Each fingerprint consists of `NUM_HASHES` minimum hash values computed
/// using different hash permutations. Two fingerprints can be compared to
/// estimate the Jaccard similarity of their underlying token sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub hashes: [u64; NUM_HASHES],
}

impl Fingerprint {
    /// Compute a MinHash fingerprint from structural tokens.
    ///
    /// For each of the `NUM_HASHES` permutations, we compute a permuted hash
    /// of every token and keep the minimum. The resulting array of minimums
    /// forms the fingerprint.
    pub fn from_tokens(tokens: &[&str]) -> Self {
        let mut hashes = [u64::MAX; NUM_HASHES];
        for token in tokens {
            let token_hash = hash_token(token);
            for (i, h) in hashes.iter_mut().enumerate() {
                let permuted = token_hash.wrapping_mul(PRIMES[i]).wrapping_add(OFFSETS[i]);
                if permuted < *h {
                    *h = permuted;
                }
            }
        }
        Fingerprint { hashes }
    }

    /// Compute Jaccard similarity estimate between two fingerprints.
    ///
    /// Returns the fraction of hash slots where both fingerprints agree,
    /// which is an unbiased estimator of the Jaccard index of the original
    /// token sets.
    pub fn similarity(&self, other: &Fingerprint) -> f64 {
        let matching = self
            .hashes
            .iter()
            .zip(other.hashes.iter())
            .filter(|(a, b)| a == b)
            .count();
        matching as f64 / NUM_HASHES as f64
    }
}

/// Tokenize source code into structural tokens suitable for fingerprinting.
///
/// Strips single-line comments (`//`, `#`), block comments (`/* ... */`),
/// and string literals (double-quoted and single-quoted). Splits the remaining
/// text on whitespace and common punctuation, returning only non-empty tokens.
pub fn structural_tokens(source: &str) -> Vec<&str> {
    // First, identify byte ranges that are NOT inside comments or string literals.
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut mask = vec![true; len]; // true = keep, false = strip

    let mut i = 0;
    while i < len {
        match bytes[i] {
            // Block comment: /* ... */
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                mask[i] = false;
                mask[i + 1] = false;
                i += 2;
                while i < len {
                    if bytes[i] == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                        mask[i] = false;
                        mask[i + 1] = false;
                        i += 2;
                        break;
                    }
                    mask[i] = false;
                    i += 1;
                }
            }
            // Single-line comment: //
            b'/' if i + 1 < len && bytes[i + 1] == b'/' => {
                mask[i] = false;
                mask[i + 1] = false;
                i += 2;
                while i < len && bytes[i] != b'\n' {
                    mask[i] = false;
                    i += 1;
                }
            }
            // Hash comment: # (Python, Ruby, etc.)
            b'#' => {
                mask[i] = false;
                i += 1;
                while i < len && bytes[i] != b'\n' {
                    mask[i] = false;
                    i += 1;
                }
            }
            // Double-quoted string literal
            b'"' => {
                mask[i] = false;
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' && i + 1 < len {
                        mask[i] = false;
                        mask[i + 1] = false;
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        mask[i] = false;
                        i += 1;
                        break;
                    }
                    mask[i] = false;
                    i += 1;
                }
            }
            // Single-quoted string literal
            b'\'' => {
                mask[i] = false;
                i += 1;
                while i < len {
                    if bytes[i] == b'\\' && i + 1 < len {
                        mask[i] = false;
                        mask[i + 1] = false;
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'\'' {
                        mask[i] = false;
                        i += 1;
                        break;
                    }
                    mask[i] = false;
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    // Now split the kept regions into tokens on whitespace and punctuation.
    // We return slices into the original source.
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;

    for (idx, &keep) in mask.iter().enumerate() {
        let ch = bytes[idx];
        let is_structural = keep && !is_separator(ch);

        if is_structural {
            if start.is_none() {
                start = Some(idx);
            }
        } else if let Some(s) = start.take() {
            let tok = &source[s..idx];
            if !tok.is_empty() {
                tokens.push(tok);
            }
        }
    }
    // Flush trailing token
    if let Some(s) = start {
        let tok = &source[s..len];
        if !tok.is_empty() {
            tokens.push(tok);
        }
    }

    tokens
}

/// Returns true if the byte is whitespace or a punctuation separator.
#[inline]
fn is_separator(b: u8) -> bool {
    matches!(
        b,
        b' ' | b'\t'
            | b'\n'
            | b'\r'
            | b'('
            | b')'
            | b'{'
            | b'}'
            | b'['
            | b']'
            | b';'
            | b','
            | b':'
            | b'.'
            | b'='
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'<'
            | b'>'
            | b'&'
            | b'|'
            | b'!'
            | b'~'
            | b'^'
            | b'%'
            | b'@'
    )
}

/// Hash a single token using `DefaultHasher`.
pub fn hash_token(token: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

/// Pre-computed prime numbers for hash permutations (128 entries).
static PRIMES: [u64; NUM_HASHES] = [
    2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97,
    101, 103, 107, 109, 113, 127, 131, 137, 139, 149, 151, 157, 163, 167, 173, 179, 181, 191, 193,
    197, 199, 211, 223, 227, 229, 233, 239, 241, 251, 257, 263, 269, 271, 277, 281, 283, 293, 307,
    311, 313, 317, 331, 337, 347, 349, 353, 359, 367, 373, 379, 383, 389, 397, 401, 409, 419, 421,
    431, 433, 439, 443, 449, 457, 461, 463, 467, 479, 487, 491, 499, 503, 509, 521, 523, 541, 547,
    557, 563, 569, 571, 577, 587, 593, 599, 601, 607, 613, 617, 619, 631, 641, 643, 647, 653, 659,
    661, 673, 677, 683, 691, 701, 709, 719,
];

/// Pre-computed offsets for hash permutations (128 entries).
/// These are large, spread-out values to ensure good hash distribution.
static OFFSETS: [u64; NUM_HASHES] = [
    0x9E3779B97F4A7C15,
    0x6C62272E07BB0142,
    0xBF58476D1CE4E5B9,
    0x94D049BB133111EB,
    0xC6A4A7935BD1E995,
    0xE7037ED1A0B428DB,
    0x8A5CD789635D2DFF,
    0x5AD8A5CD79635D2D,
    0x1B03738712FAD5C9,
    0x7F4A7C15F39CC060,
    0x2E07BB014262B2A1,
    0xD1CE4E5B9F3B7D42,
    0xBB133111EB85A2B6,
    0xD1E995C6A4A79353,
    0xA0B428DBE7037ED1,
    0x635D2DFF8A5CD789,
    0x9635D2D5AD8A5CD7,
    0x12FAD5C91B037387,
    0xF39CC0607F4A7C15,
    0x4262B2A12E07BB01,
    0x9F3B7D42D1CE4E5B,
    0xEB85A2B6BB133111,
    0xA4A79353D1E995C6,
    0xE7037ED1A0B428DB,
    0x8A5CD789635D2DFF,
    0x5AD8A5CD79635D2D,
    0x1B03738712FAD5C9,
    0x7F4A7C15F39CC060,
    0x2E07BB014262B2A1,
    0xD1CE4E5B9F3B7D42,
    0xBB133111EB85A2B6,
    0xD1E995C6A4A79353,
    0x517CC1B727220A94,
    0xA2F9836E4E441529,
    0xF47D4D9EC1A12CE5,
    0x45FA6A7D3E2E39A0,
    0x9777875A8B5B465B,
    0xE8F4A437D8884D16,
    0x3A71C114259B59D1,
    0x8BEEDE0172AE668C,
    0xDD6BFADEBFC17347,
    0x2EE917BC0CD48002,
    0x806634995A0794BD,
    0xD1E35176A7349178,
    0x2360AE53F4619E33,
    0x74DDCB31416EAAEE,
    0xC65AE80E8E9BB7A9,
    0x17D80AEBDBC8C464,
    0x695527C928F5D11F,
    0xBAD244A6760ADDDA,
    0x0C4F61839F39EA95,
    0x5DCC7E60EC6CF750,
    0xAF499B3E3999040B,
    0x00C6B81B86CC10C6,
    0x5243D4F8D3FF1D81,
    0xA3C0F1D620F22A3C,
    0xF53E0EB36E254CF7,
    0x46BB2B90BB5859B2,
    0x983848AE0E8B666D,
    0xE9B565CB5BB87328,
    0x3B3282A8A8E57FE3,
    0x8CAF9F85F612AC9E,
    0xDE2CBC634345B959,
    0x2FA9D940907AC614,
    0x8126F61DDDADD2CF,
    0xD2A4132B2AE0DF8A,
    0x24213008780DEC45,
    0x759E4CE5C53AF900,
    0xC71B69C31267F5BB,
    0x189886A05F950276,
    0x6A15A37DACC20F31,
    0xBB92C05AF9EF1BEC,
    0x0D0FDD384718F8A7,
    0x5E8CFA15943C3562,
    0xB00A16F2E169421D,
    0x018733D02E964ED8,
    0x530450AD7BC35B93,
    0xA4816D8AC8F0684E,
    0xF5FE8A6816237509,
    0x477BA7456350A1C4,
    0x98F8C422B07DAE7F,
    0xEA75E0FFFDAAAB3A,
    0x3BF2FDDD4AD7B7F5,
    0x8D701ABA980494B0,
    0xDEED37B7E531A16B,
    0x306A5495325EAE26,
    0x81E77172808BBAE1,
    0xD36490AFCCBBC79C,
    0x24E1AD8D19E8D457,
    0x765ECA6A6715E112,
    0xC7DBE747B442EDCD,
    0x1959042501700A88,
    0x6AD6210E4E9D1743,
    0xBC533DEB9BCA23FE,
    0x0DD05AC8E8F730B9,
    0x5F4D77A636243D74,
    0xB0CA948383514A2F,
    0x0247B160D07E56EA,
    0x53C4CE3E1DAB63A5,
    0xA541EB1B6AD87060,
    0xF6BF07F8B805AD1B,
    0x483C24D605329AD6,
    0x99B941B3525FA791,
    0xEB365E909F8CB44C,
    0x3CB37B6DECB9C107,
    0x8E30984B39E6CDC2,
    0xDFADB52887140A7D,
    0x312AD205D4411738,
    0x82A7EEE32174F3F3,
    0xD4250BC06EA200AE,
    0x25A2289DBBCF0D69,
    0x771F457B08FC1A24,
    0xC89C625856292EDF,
    0x1A197F35A356339A,
    0x6B969C12F0834055,
    0xBD13B8F03DB04D10,
    0x0E90D5CD8ADD59CB,
    0x601DF2AAD80A6686,
    0xB19B0F8825378341,
    0x03182C6572649FFC,
    0x549549429FAD9CB7,
    0xA612661DECBAA972,
    0xF78F82FB39E7B62D,
    0x490C9FD88714C2E8,
    0x9A89BCC5D441CFA3,
    0xEC06D9A32174DC5E,
    0x3D83F68070A1E919,
    0x8F0113ADBDCEF5D4,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_tokens_have_similarity_one() {
        let tokens = vec![
            "fn", "compute", "let", "result", "return", "value", "if", "match",
        ];
        let fp1 = Fingerprint::from_tokens(&tokens);
        let fp2 = Fingerprint::from_tokens(&tokens);
        assert_eq!(fp1.similarity(&fp2), 1.0);
    }

    #[test]
    fn same_fingerprint_has_similarity_one() {
        let tokens: Vec<&str> = vec!["alpha", "beta", "gamma", "delta"];
        let fp = Fingerprint::from_tokens(&tokens);
        assert_eq!(fp.similarity(&fp), 1.0);
    }

    #[test]
    fn completely_unrelated_tokens_have_low_similarity() {
        let tokens_a = vec![
            "fn", "compute", "let", "result", "return", "value", "match", "arm", "struct", "impl",
            "self", "new", "push", "iter", "map", "collect",
        ];
        let tokens_b = vec![
            "class",
            "constructor",
            "this",
            "prototype",
            "window",
            "document",
            "querySelector",
            "addEventListener",
            "innerHTML",
            "style",
            "display",
            "flex",
            "import",
            "export",
            "default",
            "module",
        ];
        let fp_a = Fingerprint::from_tokens(&tokens_a);
        let fp_b = Fingerprint::from_tokens(&tokens_b);
        let sim = fp_a.similarity(&fp_b);
        assert!(
            sim < 0.3,
            "Expected similarity < 0.3 for unrelated token sets, got {}",
            sim
        );
    }

    #[test]
    fn structural_tokens_strips_line_comments() {
        let source = "let x = 10; // this is a comment\nlet y = 20;";
        let tokens = structural_tokens(source);
        assert!(tokens.contains(&"x"));
        assert!(tokens.contains(&"y"));
        // "this", "is", "a", "comment" should not appear
        assert!(!tokens.contains(&"this"));
        assert!(!tokens.contains(&"comment"));
    }

    #[test]
    fn structural_tokens_strips_hash_comments() {
        let source = "x = 10\n# this is a python comment\ny = 20";
        let tokens = structural_tokens(source);
        assert!(tokens.contains(&"x"));
        assert!(tokens.contains(&"y"));
        assert!(!tokens.contains(&"python"));
        assert!(!tokens.contains(&"comment"));
    }

    #[test]
    fn structural_tokens_strips_block_comments() {
        let source = "let a = 1; /* block comment with if while for */ let b = 2;";
        let tokens = structural_tokens(source);
        assert!(tokens.contains(&"let"));
        assert!(tokens.contains(&"a"));
        assert!(tokens.contains(&"b"));
        assert!(!tokens.contains(&"block"));
        assert!(!tokens.contains(&"comment"));
    }

    #[test]
    fn structural_tokens_strips_double_quoted_strings() {
        let source = r#"let msg = "hello world"; let x = 5;"#;
        let tokens = structural_tokens(source);
        assert!(tokens.contains(&"let"));
        assert!(tokens.contains(&"msg"));
        assert!(tokens.contains(&"x"));
        assert!(!tokens.contains(&"hello"));
        assert!(!tokens.contains(&"world"));
    }

    #[test]
    fn structural_tokens_strips_single_quoted_strings() {
        let source = "let msg = 'hello world'; let x = 5;";
        let tokens = structural_tokens(source);
        assert!(tokens.contains(&"let"));
        assert!(tokens.contains(&"msg"));
        assert!(!tokens.contains(&"hello"));
        assert!(!tokens.contains(&"world"));
    }

    #[test]
    fn structural_tokens_handles_escaped_quotes_in_strings() {
        let source = r#"let s = "he said \"hi\""; let y = 1;"#;
        let tokens = structural_tokens(source);
        assert!(tokens.contains(&"let"));
        assert!(tokens.contains(&"s"));
        assert!(tokens.contains(&"y"));
        assert!(!tokens.contains(&"said"));
        assert!(!tokens.contains(&"hi"));
    }

    #[test]
    fn empty_tokens_produce_max_hashes() {
        let fp = Fingerprint::from_tokens(&[]);
        for &h in &fp.hashes {
            assert_eq!(h, u64::MAX);
        }
    }
}
