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
pub fn verify_state(received: &str, expected: &str) -> bool {
    // Constant-time comparison would be ideal but for state params
    // timing attacks are not practical. Simple equality suffices.
    received == expected
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
}
