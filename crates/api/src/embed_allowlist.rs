// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P6 piece A — URL allowlist for sandboxed embeds.
//!
//! Each known provider (YouTube, Vimeo, Figma, Loom, CodeSandbox)
//! ships its own URL pattern + iframe-friendly host so the
//! `NodeType::Embed` block can be rendered as an iframe with a
//! valid `src` and a CSP `frame-src` entry. The `Generic` variant
//! defers the per-domain allow-list decision to the workspace
//! (via `Workspace.embed_allowed_domains`); v1 ships with that
//! field unset, so `Generic` URLs are rejected at insert.
//!
//! Why string-pattern matching rather than a regex crate: the
//! patterns are small, the input is short, and we don't want a
//! workspace-wide `regex` dep just for six URL shapes. Each
//! provider's `detect`/`embed_src` pair fits in a few lines.
//!
//! The output of `validate_url` is a `(EmbedProvider, String)`
//! pair: the provider tag is stored in the cell attribute; the
//! string is the iframe-ready URL (e.g. `youtube.com/watch?v=ABC`
//! gets rewritten to `youtube.com/embed/ABC`, which is what the
//! iframe actually needs to load). Storing the rewritten URL
//! keeps the renderer dumb.

use std::collections::HashSet;

/// Known providers that ship with default URL patterns and CSP
/// entries. `Generic { domain }` defers the allow decision to the
/// workspace's `embed_allowed_domains`; the iframe still loads
/// the original URL verbatim, no rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedProvider {
    YouTube,
    Vimeo,
    Figma,
    Loom,
    CodeSandbox,
    /// Generic embed against a workspace-allowlisted host.
    /// `domain` is the hostname-only form (`figjam.example.com`),
    /// no scheme, no path.
    Generic { domain: String },
}

impl EmbedProvider {
    /// Wire-format string used as the `provider` cell attribute.
    /// Mirrors the round-trip pattern in `crates/collab/src/schema.rs`'s
    /// tag-name dance: the `to_attr` writer and the `from_attr`
    /// parser stay symmetric so a doc serialized in v1 can be
    /// loaded by any later version that knows the enum.
    pub fn to_attr(&self) -> String {
        match self {
            EmbedProvider::YouTube => "youtube".to_string(),
            EmbedProvider::Vimeo => "vimeo".to_string(),
            EmbedProvider::Figma => "figma".to_string(),
            EmbedProvider::Loom => "loom".to_string(),
            EmbedProvider::CodeSandbox => "codesandbox".to_string(),
            EmbedProvider::Generic { domain } => format!("generic:{domain}"),
        }
    }

    /// Parse a `provider` attribute back into the enum. `None` if
    /// the attribute is malformed (e.g. an empty `generic:` tail).
    pub fn from_attr(s: &str) -> Option<Self> {
        match s {
            "youtube" => Some(Self::YouTube),
            "vimeo" => Some(Self::Vimeo),
            "figma" => Some(Self::Figma),
            "loom" => Some(Self::Loom),
            "codesandbox" => Some(Self::CodeSandbox),
            _ => s
                .strip_prefix("generic:")
                .filter(|d| !d.is_empty())
                .map(|d| Self::Generic { domain: d.to_string() }),
        }
    }

    /// Default iframe height for fresh embeds of this provider.
    /// The user can resize after insert (200..1200 clamped). Tuned
    /// per provider's typical content shape — a YouTube video at
    /// 315 px fits the 16:9 ratio at 560 px width, etc.
    pub fn default_height(&self) -> u32 {
        match self {
            EmbedProvider::YouTube => 315,
            EmbedProvider::Vimeo => 360,
            EmbedProvider::Figma => 450,
            EmbedProvider::Loom => 360,
            EmbedProvider::CodeSandbox => 500,
            EmbedProvider::Generic { .. } => 400,
        }
    }
}

/// Why a URL was rejected. Stable enum so route handlers can map
/// each variant to the right HTTP status (400 for malformed,
/// 403 for domain-not-allowed, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedRejection {
    /// URL didn't start with `https://`. We don't accept http: for
    /// embeds (iframe over http on an https doc would mixed-content
    /// block anyway).
    NotHttps,
    /// URL matched no known provider pattern AND the workspace's
    /// `embed_allowed_domains` doesn't list the host. Also covers a
    /// host that matches a provider but whose id couldn't be extracted
    /// (e.g. `youtube.com` with no `v=` parameter) — `validate_url`
    /// falls through to this rather than distinguishing the two.
    UnknownProvider,
}

/// Validate `url` against the known providers + the workspace's
/// `embed_allowed_domains` allowlist (passed in as a set of
/// lowercased hostname strings). On success returns the provider
/// tag plus the iframe-ready URL (which may differ from the input
/// when we rewrite YouTube watch URLs to embed URLs, etc.).
pub fn validate_url(
    url: &str,
    workspace_allowed_domains: &HashSet<String>,
) -> Result<(EmbedProvider, String), EmbedRejection> {
    let url = url.trim();
    if !url.starts_with("https://") {
        return Err(EmbedRejection::NotHttps);
    }

    // Provider-specific patterns, in priority order. Each `try_*`
    // returns Some(rewritten_url) when the input matches; we
    // short-circuit on the first hit.
    if let Some(src) = try_youtube(url) {
        return Ok((EmbedProvider::YouTube, src));
    }
    if let Some(src) = try_vimeo(url) {
        return Ok((EmbedProvider::Vimeo, src));
    }
    if let Some(src) = try_figma(url) {
        return Ok((EmbedProvider::Figma, src));
    }
    if let Some(src) = try_loom(url) {
        return Ok((EmbedProvider::Loom, src));
    }
    if let Some(src) = try_codesandbox(url) {
        return Ok((EmbedProvider::CodeSandbox, src));
    }

    // Fall-through: generic per-workspace allowlist. Hostname
    // extraction is naive (https://HOST/...) — good enough for
    // the allow-check, and the iframe ultimately loads the full
    // URL verbatim.
    let host = match extract_host(url) {
        Some(h) => h.to_lowercase(),
        None => return Err(EmbedRejection::UnknownProvider),
    };
    if workspace_allowed_domains.contains(&host) {
        return Ok((EmbedProvider::Generic { domain: host }, url.to_string()));
    }
    Err(EmbedRejection::UnknownProvider)
}

// ─── Per-provider matchers ──────────────────────────────────────

fn try_youtube(url: &str) -> Option<String> {
    // Accepted forms:
    //   https://www.youtube.com/watch?v=ABC
    //   https://youtube.com/watch?v=ABC
    //   https://youtu.be/ABC
    //   https://www.youtube.com/embed/ABC
    let candidates = [
        ("https://www.youtube.com/watch?v=", true),
        ("https://youtube.com/watch?v=", true),
        ("https://m.youtube.com/watch?v=", true),
    ];
    for (prefix, rewrite_to_embed) in candidates {
        if let Some(rest) = url.strip_prefix(prefix) {
            // Truncate at the first `&` so trailing tracking params
            // don't end up in the embed URL.
            let id = rest.split('&').next().unwrap_or(rest);
            if !is_alnum_or_dash(id) || id.len() > 32 {
                continue;
            }
            return Some(if rewrite_to_embed {
                format!("https://www.youtube.com/embed/{id}")
            } else {
                url.to_string()
            });
        }
    }
    if let Some(rest) = url.strip_prefix("https://youtu.be/") {
        let id = rest.split(['?', '&']).next().unwrap_or(rest);
        if is_alnum_or_dash(id) && id.len() <= 32 {
            return Some(format!("https://www.youtube.com/embed/{id}"));
        }
    }
    if let Some(rest) = url.strip_prefix("https://www.youtube.com/embed/") {
        let id = rest.split(['?', '&']).next().unwrap_or(rest);
        if is_alnum_or_dash(id) && id.len() <= 32 {
            return Some(format!("https://www.youtube.com/embed/{id}"));
        }
    }
    None
}

fn try_vimeo(url: &str) -> Option<String> {
    // Accepted: https://vimeo.com/123, https://player.vimeo.com/video/123
    if let Some(rest) = url.strip_prefix("https://vimeo.com/") {
        let id = rest.split(['?', '/']).next().unwrap_or(rest);
        if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
            return Some(format!("https://player.vimeo.com/video/{id}"));
        }
    }
    if let Some(rest) = url.strip_prefix("https://player.vimeo.com/video/") {
        let id = rest.split(['?', '/']).next().unwrap_or(rest);
        if id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty() {
            return Some(format!("https://player.vimeo.com/video/{id}"));
        }
    }
    None
}

fn try_figma(url: &str) -> Option<String> {
    // Accepted: https://www.figma.com/{file,design,embed}/...
    // figma's embed endpoint takes the original URL as a query
    // param: https://www.figma.com/embed?embed_host=share&url=<encoded>
    for path in ["/file/", "/design/", "/embed/"] {
        if url.starts_with("https://www.figma.com") && url.contains(path) {
            // Already in embed form? Pass through.
            if url.contains("/embed?") || url.starts_with("https://www.figma.com/embed/") {
                return Some(url.to_string());
            }
            // Wrap a /file/... or /design/... URL into the embed form.
            return Some(format!(
                "https://www.figma.com/embed?embed_host=ogrenotes&url={}",
                url_encode(url),
            ));
        }
    }
    None
}

fn try_loom(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("https://www.loom.com/share/") {
        let id = rest.split(['?', '/']).next().unwrap_or(rest);
        if is_alnum_or_dash(id) && !id.is_empty() {
            return Some(format!("https://www.loom.com/embed/{id}"));
        }
    }
    if let Some(rest) = url.strip_prefix("https://www.loom.com/embed/") {
        let id = rest.split(['?', '/']).next().unwrap_or(rest);
        if is_alnum_or_dash(id) && !id.is_empty() {
            return Some(format!("https://www.loom.com/embed/{id}"));
        }
    }
    None
}

fn try_codesandbox(url: &str) -> Option<String> {
    // CodeSandbox embed URLs are `https://codesandbox.io/embed/<id>`;
    // their share URLs are `https://codesandbox.io/s/<id>`. Both
    // accept rewriting to the embed form.
    if let Some(rest) = url.strip_prefix("https://codesandbox.io/embed/") {
        let id = rest.split(['?', '/']).next().unwrap_or(rest);
        if is_alnum_or_dash(id) && !id.is_empty() {
            return Some(format!("https://codesandbox.io/embed/{id}"));
        }
    }
    if let Some(rest) = url.strip_prefix("https://codesandbox.io/s/") {
        let id = rest.split(['?', '/']).next().unwrap_or(rest);
        if is_alnum_or_dash(id) && !id.is_empty() {
            return Some(format!("https://codesandbox.io/embed/{id}"));
        }
    }
    None
}

// ─── Helpers ────────────────────────────────────────────────────

fn is_alnum_or_dash(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Extract the host portion of `https://HOST/...`. Strips the scheme,
/// any user-info, and the path. Returns None if the URL doesn't start
/// with `https://`.
fn extract_host(url: &str) -> Option<&str> {
    let after_scheme = url.strip_prefix("https://")?;
    // Strip user-info if any (won't have one for our trusted set, but
    // defensive).
    let after_auth = after_scheme.rsplit_once('@').map_or(after_scheme, |(_, h)| h);
    // Truncate at the first `/` or `?` or `#`.
    let host_end = after_auth
        .find(|c: char| c == '/' || c == '?' || c == '#')
        .unwrap_or(after_auth.len());
    let host = &after_auth[..host_end];
    if host.is_empty() { None } else { Some(host) }
}

/// Minimal URL-encoder for the small set of characters that show
/// up in the Figma embed wrap-around — colon, slash, question
/// mark, ampersand, hash, equals. Avoids pulling `urlencoding`
/// for a single call site.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            _ => {
                let mut buf = [0u8; 4];
                let bytes = c.encode_utf8(&mut buf).as_bytes();
                for b in bytes {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_allowed() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn rejects_http() {
        let r = validate_url("http://www.youtube.com/watch?v=abc", &no_allowed());
        assert_eq!(r, Err(EmbedRejection::NotHttps));
    }

    #[test]
    fn youtube_watch_rewrites_to_embed() {
        let (p, src) = validate_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &no_allowed(),
        )
        .unwrap();
        assert_eq!(p, EmbedProvider::YouTube);
        assert_eq!(src, "https://www.youtube.com/embed/dQw4w9WgXcQ");
    }

    #[test]
    fn youtube_short_url_rewrites() {
        let (p, src) =
            validate_url("https://youtu.be/dQw4w9WgXcQ", &no_allowed()).unwrap();
        assert_eq!(p, EmbedProvider::YouTube);
        assert_eq!(src, "https://www.youtube.com/embed/dQw4w9WgXcQ");
    }

    #[test]
    fn youtube_strips_tracking_params() {
        let (_, src) = validate_url(
            "https://www.youtube.com/watch?v=abc123&t=42s&feature=share",
            &no_allowed(),
        )
        .unwrap();
        assert_eq!(src, "https://www.youtube.com/embed/abc123");
    }

    #[test]
    fn youtube_missing_v_rejected() {
        let r = validate_url("https://www.youtube.com/watch?foo=bar", &no_allowed());
        assert_eq!(r, Err(EmbedRejection::UnknownProvider));
    }

    #[test]
    fn vimeo_rewrites_to_player() {
        let (p, src) = validate_url("https://vimeo.com/76979871", &no_allowed()).unwrap();
        assert_eq!(p, EmbedProvider::Vimeo);
        assert_eq!(src, "https://player.vimeo.com/video/76979871");
    }

    #[test]
    fn vimeo_non_numeric_rejected() {
        let r = validate_url("https://vimeo.com/not-a-number", &no_allowed());
        assert_eq!(r, Err(EmbedRejection::UnknownProvider));
    }

    #[test]
    fn figma_file_url_wraps_into_embed() {
        let (p, src) = validate_url(
            "https://www.figma.com/file/abc/design",
            &no_allowed(),
        )
        .unwrap();
        assert_eq!(p, EmbedProvider::Figma);
        assert!(src.starts_with("https://www.figma.com/embed?"));
        assert!(src.contains("url=https%3A%2F%2Fwww.figma.com%2Ffile%2Fabc%2Fdesign"));
    }

    #[test]
    fn loom_share_rewrites_to_embed() {
        let (p, src) =
            validate_url("https://www.loom.com/share/abc123def", &no_allowed()).unwrap();
        assert_eq!(p, EmbedProvider::Loom);
        assert_eq!(src, "https://www.loom.com/embed/abc123def");
    }

    #[test]
    fn codesandbox_share_rewrites_to_embed() {
        let (p, src) =
            validate_url("https://codesandbox.io/s/abc123", &no_allowed()).unwrap();
        assert_eq!(p, EmbedProvider::CodeSandbox);
        assert_eq!(src, "https://codesandbox.io/embed/abc123");
    }

    #[test]
    fn generic_domain_allowed_by_workspace() {
        let mut allowed = HashSet::new();
        allowed.insert("internal.example.com".to_string());
        let (p, src) = validate_url(
            "https://internal.example.com/dashboards/42",
            &allowed,
        )
        .unwrap();
        assert!(matches!(p, EmbedProvider::Generic { ref domain } if domain == "internal.example.com"));
        assert_eq!(src, "https://internal.example.com/dashboards/42");
    }

    #[test]
    fn generic_domain_not_allowed_rejects() {
        let r = validate_url("https://random.example.com/page", &no_allowed());
        assert_eq!(r, Err(EmbedRejection::UnknownProvider));
    }

    // ── SSRF / allowlist-boundary hardening ──────────────────────
    // The host extracted for the allow-check must be the *real* host,
    // not a look-alike spoofed via userinfo, a sub-domain suffix, or
    // case. A regression in `extract_host` here is an allowlist bypass.

    #[test]
    fn host_spoof_via_userinfo_at_is_rejected() {
        // `internal.example.com@evil.com` actually targets evil.com.
        let mut allowed = HashSet::new();
        allowed.insert("internal.example.com".to_string());
        let r = validate_url("https://internal.example.com@evil.com/x", &allowed);
        assert_eq!(
            r,
            Err(EmbedRejection::UnknownProvider),
            "userinfo `@` must not let an allowlisted name front a foreign host"
        );
    }

    #[test]
    fn provider_prefix_spoof_is_rejected() {
        // Neither a suffix sub-domain nor a userinfo prefix may match a
        // provider; both fall through to the (empty) generic allowlist.
        let suffix = validate_url("https://www.youtube.com.evil.com/watch?v=abc", &no_allowed());
        assert_eq!(suffix, Err(EmbedRejection::UnknownProvider));
        let userinfo = validate_url("https://www.youtube.com@evil.com/watch?v=abc", &no_allowed());
        assert_eq!(userinfo, Err(EmbedRejection::UnknownProvider));
    }

    #[test]
    fn generic_host_match_is_case_insensitive() {
        // The allowlist holds lowercase hosts; an upper/mixed-case URL
        // host must still match (extract_host lowercases). A regression
        // dropping the lowercase would silently deny legitimate embeds.
        let mut allowed = HashSet::new();
        allowed.insert("internal.example.com".to_string());
        let (p, _) = validate_url("https://INTERNAL.Example.COM/x", &allowed).unwrap();
        assert!(matches!(p, EmbedProvider::Generic { ref domain } if domain == "internal.example.com"));
    }

    #[test]
    fn provider_attr_round_trips() {
        let cases = [
            EmbedProvider::YouTube,
            EmbedProvider::Vimeo,
            EmbedProvider::Figma,
            EmbedProvider::Loom,
            EmbedProvider::CodeSandbox,
            EmbedProvider::Generic { domain: "x.example.com".into() },
        ];
        for p in cases {
            let s = p.to_attr();
            let parsed = EmbedProvider::from_attr(&s).expect(&s);
            assert_eq!(parsed, p);
        }
    }

    #[test]
    fn generic_attr_with_empty_domain_rejects() {
        assert_eq!(EmbedProvider::from_attr("generic:"), None);
    }

    #[test]
    fn extract_host_strips_path() {
        assert_eq!(
            extract_host("https://example.com/a/b?c=1"),
            Some("example.com"),
        );
    }
}
