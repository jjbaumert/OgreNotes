// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// PKCE code verifier and challenge pair.
#[derive(Debug, Clone)]
pub struct PkceChallenge {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generate a PKCE code verifier and S256 challenge.
pub fn generate_pkce() -> PkceChallenge {
    // RFC 7636: verifier is 43-128 characters from unreserved set
    let verifier: String = (0..64)
        .map(|_| {
            let idx = rand::random_range(0..PKCE_ALPHABET.len());
            PKCE_ALPHABET[idx] as char
        })
        .collect();

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    let challenge = URL_SAFE_NO_PAD.encode(hash);

    PkceChallenge {
        code_verifier: verifier,
        code_challenge: challenge,
    }
}

/// Characters allowed in PKCE code verifier (RFC 7636 Appendix B).
const PKCE_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

/// Generate a random state parameter for CSRF protection.
pub fn generate_state() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the OAuth authorization URL with properly percent-encoded parameters.
pub fn build_authorization_url(
    authorize_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceChallenge,
    state: &str,
    scopes: &[&str],
) -> String {
    let scope = scopes.join(" ");
    format!(
        "{authorize_endpoint}?\
         client_id={}&\
         redirect_uri={}&\
         response_type=code&\
         code_challenge={}&\
         code_challenge_method=S256&\
         state={}&\
         scope={}",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(&pkce.code_challenge),
        urlencoding::encode(state),
        urlencoding::encode(&scope),
    )
}

/// Verify that a received state matches the expected state.
///
/// An empty state never verifies (issue #13): a caller that maps
/// "no stored state" to `""` on both sides must fail CSRF
/// verification, not pass it — safe by construction rather than
/// relying on every caller to reject absent state first.
pub fn verify_state(received: &str, expected: &str) -> bool {
    // Constant-time comparison would be ideal but for state params
    // timing attacks are not practical. Simple equality suffices.
    !received.is_empty() && received == expected
}

/// Hash a refresh token for storage (never store plaintext).
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

/// Generate a random refresh token.
pub fn generate_refresh_token() -> String {
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_length() {
        let pkce = generate_pkce();
        let len = pkce.code_verifier.len();
        assert!(
            (43..=128).contains(&len),
            "verifier length {len} not in 43..=128"
        );
    }

    #[test]
    fn pkce_challenge_is_s256() {
        let pkce = generate_pkce();

        // Independently compute the challenge
        let mut hasher = Sha256::new();
        hasher.update(pkce.code_verifier.as_bytes());
        let hash = hasher.finalize();
        let expected = URL_SAFE_NO_PAD.encode(hash);

        assert_eq!(pkce.code_challenge, expected);
    }

    #[test]
    fn authorization_url_contains_required_params() {
        let pkce = generate_pkce();
        let state = generate_state();

        let url = build_authorization_url(
            "https://github.com/login/oauth/authorize",
            "my_client_id",
            "http://localhost:3000/api/v1/auth/callback",
            &pkce,
            &state,
            &["user:email"],
        );

        assert!(url.contains("client_id=my_client_id"));
        assert!(url.contains("redirect_uri="));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("state={state}")));
    }

    #[test]
    fn state_mismatch_rejected() {
        let state = generate_state();
        assert!(verify_state(&state, &state));
        assert!(!verify_state(&state, "wrong-state"));
    }

    #[test]
    fn hash_token_is_not_plaintext() {
        let token = generate_refresh_token();
        let hash = hash_token(&token);
        assert_ne!(token, hash);
    }

    #[test]
    fn hash_token_is_deterministic() {
        let token = "test-token-value";
        let hash1 = hash_token(token);
        let hash2 = hash_token(token);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn refresh_token_is_long_enough() {
        let token = generate_refresh_token();
        // 32 bytes base64url encoded = 43 characters
        assert!(token.len() >= 43);
    }

    #[test]
    fn pkce_verifier_characters_are_valid() {
        for _ in 0..100 {
            let pkce = generate_pkce();
            for ch in pkce.code_verifier.chars() {
                assert!(
                    ch.is_ascii_alphanumeric() || ch == '-' || ch == '.' || ch == '_' || ch == '~',
                    "invalid PKCE character: {ch}"
                );
            }
        }
    }

    // ─── Injection resistance, entropy, and algorithm pinning ───────

    #[test]
    fn authorization_url_resists_parameter_injection() {
        // Values containing '&' and '=' must arrive percent-encoded so a
        // malicious value can't smuggle extra query parameters (e.g. a
        // second redirect_uri) into the authorize request.
        let pkce = generate_pkce();
        let url = build_authorization_url(
            "https://idp.example/authorize",
            "evil&redirect_uri=https://attacker.example",
            "http://localhost:3000/cb?x=1&y=2",
            &pkce,
            "state&scope=admin",
            &["user:email"],
        );
        let query = url.split_once('?').expect("url has a query").1;
        // Exactly the 7 parameters we set — nothing smuggled in.
        assert_eq!(query.split('&').count(), 7, "unexpected params in {url}");
        assert!(!url.contains("redirect_uri=https://attacker.example"));
        assert!(url.contains("client_id=evil%26redirect_uri%3D"));
        assert!(url.contains("state=state%26scope%3Dadmin"));
    }

    #[test]
    fn authorization_url_encodes_multi_scope_list() {
        // Scopes join with a space, which must be %20 in the query, and
        // the ':' inside each scope must be percent-encoded too.
        let pkce = generate_pkce();
        let url = build_authorization_url(
            "https://idp.example/authorize",
            "cid",
            "http://localhost/cb",
            &pkce,
            "st",
            &["user:email", "read:org"],
        );
        assert!(url.contains("scope=user%3Aemail%20read%3Aorg"), "got {url}");
    }

    #[test]
    fn hash_token_pins_sha256_base64url() {
        // Known-answer test: hash_token must remain SHA-256 →
        // base64url-no-pad. Every stored session row depends on this
        // exact algorithm — an accidental change would invalidate all
        // refresh-token hashes (mass logout) or silently weaken them.
        assert_eq!(
            hash_token("test-token-value"),
            "vGo0hptylCKH-yD9zgkvw5LpJLTxmGtd-kf7wQHix_s"
        );
    }

    #[test]
    fn state_and_refresh_tokens_are_unique_and_url_safe() {
        // 32 bytes of fresh randomness per call: consecutive calls must
        // differ, and the encoding must be strict base64url so the value
        // is safe in a query string or cookie without further escaping.
        let (s1, s2) = (generate_state(), generate_state());
        assert_ne!(s1, s2, "states must not repeat");
        let (t1, t2) = (generate_refresh_token(), generate_refresh_token());
        assert_ne!(t1, t2, "refresh tokens must not repeat");
        for v in [&s1, &s2, &t1, &t2] {
            assert_eq!(v.len(), 43); // 32 bytes → 43 base64url chars
            assert!(
                v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "non-url-safe char in {v}"
            );
        }
    }

    #[test]
    fn pkce_pairs_are_unique_per_call() {
        let a = generate_pkce();
        let b = generate_pkce();
        assert_ne!(a.code_verifier, b.code_verifier);
        assert_ne!(a.code_challenge, b.code_challenge);
        // The challenge is a digest, never the verifier itself.
        assert_ne!(a.code_verifier, a.code_challenge);
    }

    #[test]
    fn verify_state_edge_cases() {
        // Prefix / suffix near-misses must fail.
        assert!(!verify_state("abc", "abcd"));
        assert!(!verify_state("abcd", "abc"));
        assert!(!verify_state("", "expected"));
        assert!(!verify_state("received", ""));
        // Issue #13 (deliberate behavior change): an absent state must
        // never verify. A caller that maps "no stored state" to "" on
        // both sides used to pass CSRF verification; the function is now
        // safe by construction instead of relying on every caller to
        // reject absent state first.
        assert!(!verify_state("", ""));
    }
}
