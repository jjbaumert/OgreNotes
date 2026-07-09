// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use nanoid::nanoid;

/// URL-safe alphabet for ID generation.
const ALPHABET: &[char] = &[
    'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R',
    'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j',
    'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1',
    '2', '3', '4', '5', '6', '7', '8', '9', '_', '-',
];

/// 21 characters * 6 bits/char = 126 bits of entropy.
/// Matches nanoid's default length and provides sufficient entropy
/// for share-by-link access patterns (enumeration resistance).
const ID_LEN: usize = 21;

/// Generate a new unique ID (21 chars, URL-safe, 126 bits of entropy).
pub fn new_id() -> String {
    nanoid!(ID_LEN, ALPHABET)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn id_length() {
        let id = new_id();
        assert_eq!(id.len(), ID_LEN);
    }

    #[test]
    fn id_uniqueness() {
        let ids: HashSet<String> = (0..10_000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn id_is_url_safe() {
        for _ in 0..1_000 {
            let id = new_id();
            for ch in id.chars() {
                assert!(
                    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-',
                    "unexpected character: {ch}"
                );
            }
        }
    }

    #[test]
    fn alphabet_has_64_unique_symbols() {
        // The documented 126-bit entropy claim (21 chars x 6 bits) requires
        // exactly 64 distinct symbols. A duplicated or dropped character in
        // the const would silently weaken share-link enumeration resistance
        // without failing any other test.
        assert_eq!(ALPHABET.len(), 64, "6 bits per char requires 64 symbols");
        let unique: HashSet<char> = ALPHABET.iter().copied().collect();
        assert_eq!(unique.len(), 64, "alphabet must not contain duplicates");
    }
}
