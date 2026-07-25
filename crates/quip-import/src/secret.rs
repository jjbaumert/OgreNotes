use secrecy::{ExposeSecret, Secret};

/// A Quip personal access token. Wraps `secrecy::Secret` so it zeroizes on
/// drop and never appears in Debug/logs. The ONLY way to read it is `expose`.
pub struct QuipToken(Secret<String>);

impl QuipToken {
    pub fn new(raw: String) -> Self {
        Self(Secret::new(raw))
    }

    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl std::fmt::Debug for QuipToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QuipToken([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted_and_expose_returns_value() {
        let t = QuipToken::new("secret-abc123".into());
        assert_eq!(format!("{t:?}"), "QuipToken([redacted])");
        assert!(!format!("{t:?}").contains("abc123"));
        assert_eq!(t.expose(), "secret-abc123");
    }
}
