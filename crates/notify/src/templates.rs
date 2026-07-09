// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Inline `format!` email rendering for each `NotifType` variant. Every
//! render returns `(subject, html_body, text_body)`; the subject is short
//! and the bodies include a deep link back to the document or thread.
//!
//! Kept intentionally minimal — richer HTML design is a follow-up. Tests
//! pin the subject prefix and presence of the link so refactors can't
//! silently break the deep-link shape.
//!
//! Deep links carry a signed `?notif=<sig>.<exp>` token (#40). The
//! token has two purposes:
//!   - Long-lived URL retention: an email forwarded or scraped from a
//!     corporate archive years later still shows the URL's expiry to
//!     anyone inspecting it. The expiry is bound by HMAC so it can't
//!     be lifted without the workspace's signing secret.
//!   - Future verify endpoint: a logged "open via email" handler can
//!     re-derive the HMAC and confirm the click came from a real
//!     OgreNotes-issued link (vs a phisher who guessed a doc id).
//! The frontend currently ignores the param — it's purely a durable
//! record. Phase-2 enforcement is tracked separately.

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use hmac::{Hmac, Mac};
use ogrenotes_storage::models::notification::{NotifType, Notification};
use sha2::Sha256;

/// Default expiry for email-link tokens. Long enough to survive a
/// few days of "I'll get to it later" and a typical weekend, short
/// enough that an archived URL becomes inert before a phisher can
/// usefully repurpose it. The future verify endpoint enforces this;
/// today the value is recorded in the token for that handler to
/// read once it lands.
pub const NOTIF_LINK_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// Build the signed link query parameter (`notif=<sig>.<exp>`) for
/// an email-deep-link to `target_id` (a `doc_id` or `thread_id`) on
/// behalf of `user_id`. `exp_unix` is the absolute expiry timestamp
/// in seconds.
///
/// The signed input is `"notif:v1:{user_id}:{target_id}:{exp_unix}"`.
/// `notif:v1` is the domain separator that prevents this HMAC from
/// being mistaken for any other signed payload (e.g. session
/// tokens) keyed on the same secret. Bumping the prefix to `v2`
/// retires every v1 token outstanding — useful if the signed shape
/// changes.
///
/// Returned string is the bare `notif=...` value, ready to splice
/// into a URL after `?` or `&`.
pub fn build_notif_param(
    user_id: &str,
    target_id: &str,
    exp_unix: i64,
    secret: &[u8],
) -> String {
    let input = format!("notif:v1:{user_id}:{target_id}:{exp_unix}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .expect("HMAC accepts any key length");
    mac.update(input.as_bytes());
    let sig = B64URL.encode(mac.finalize().into_bytes());
    format!("notif={sig}.{exp_unix}")
}

/// Verify an inbound `?notif=<sig>.<exp>` parameter against the
/// (`user_id`, `target_id`) tuple of the request. Returns `true`
/// iff the signature matches AND the expiry is still in the future.
///
/// Constant-time comparison — `hmac::Mac::verify_slice` rejects
/// without timing-leaking which byte mismatched. The expiry check
/// runs after verify so an attacker probing for "is this token
/// well-formed but expired" gets the same response time as a
/// well-formed-and-fresh token with a bad signature.
///
/// Provided now so the future `POST /notifications/verify-link`
/// handler can call it without re-implementing the token shape.
/// `now_unix` is injected so the function stays pure (tests pass a
/// fixed time, production passes the system clock).
pub fn verify_notif_param(
    raw_param: &str,
    user_id: &str,
    target_id: &str,
    secret: &[u8],
    now_unix: i64,
) -> bool {
    let Some((sig_b64, exp_str)) = raw_param.split_once('.') else {
        return false;
    };
    let Ok(exp_unix) = exp_str.parse::<i64>() else {
        return false;
    };
    let Ok(sig_bytes) = B64URL.decode(sig_b64) else {
        return false;
    };
    let input = format!("notif:v1:{user_id}:{target_id}:{exp_unix}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .expect("HMAC accepts any key length");
    mac.update(input.as_bytes());
    if mac.verify_slice(&sig_bytes).is_err() {
        return false;
    }
    exp_unix > now_unix
}

/// Rendered email: subject line + both HTML and plain-text bodies.
pub struct RenderedEmail {
    pub subject: String,
    pub html: String,
    pub text: String,
}

/// Render an email for a single notification. `actor_name` is the display
/// name of the user who triggered the event; `frontend_origin` is the base
/// URL used to build the doc link (e.g. `https://app.example.com`).
///
/// `notif_secret` keys the email-link HMAC (#40); `exp_unix` is the
/// absolute expiry the token is bound to. Callers should pass
/// `now_unix + NOTIF_LINK_TTL_SECS`; injected as a parameter so the
/// caller picks the clock and tests can pin it.
pub fn render(
    notif: &Notification,
    actor_name: &str,
    frontend_origin: &str,
    notif_secret: &[u8],
    exp_unix: i64,
) -> RenderedEmail {
    let link = link_for(notif, frontend_origin, notif_secret, exp_unix);
    let event = event_label(&notif.notif_type);
    let subject = format!("{actor_name} {event}");
    let text = format!(
        "{actor_name} {event}.\n\n{}\n\n{link}\n",
        notif.message
    );
    let html = format!(
        r#"<!doctype html><html><body style="font-family:sans-serif">
<p><strong>{actor}</strong> {event}.</p>
<p>{message}</p>
<p><a href="{link}">Open in OgreNotes</a></p>
</body></html>"#,
        actor = html_escape(actor_name),
        event = html_escape(event),
        message = html_escape(&notif.message),
        link = html_escape(&link),
    );
    RenderedEmail { subject, html, text }
}

fn event_label(t: &NotifType) -> &'static str {
    match t {
        NotifType::Shared => "shared a document with you",
        NotifType::Mentioned => "mentioned you",
        NotifType::Commented => "commented",
        NotifType::ChatMessage => "sent you a message",
        NotifType::DocumentEdited => "edited a document",
        NotifType::DocumentOpened => "opened your document",
        NotifType::RequestAccess => "requested edit access to your document",
    }
}

fn link_for(
    notif: &Notification,
    frontend_origin: &str,
    notif_secret: &[u8],
    exp_unix: i64,
) -> String {
    let (path_segment, target_id) = if let Some(doc_id) = &notif.doc_id {
        (Some("d"), Some(doc_id.as_str()))
    } else if let Some(thread_id) = &notif.thread_id {
        (Some("c"), Some(thread_id.as_str()))
    } else {
        (None, None)
    };
    let Some((seg, target)) = path_segment.zip(target_id) else {
        // Catch-all link (no doc / thread) — the homepage. No
        // signed token because there's nothing target-specific to
        // bind it to.
        return frontend_origin.to_string();
    };
    let param = build_notif_param(&notif.user_id, target, exp_unix, notif_secret);
    format!("{frontend_origin}/{seg}/{target}?{param}")
}

/// Minimal HTML escaper for the four characters that matter inside
/// untrusted text inserted into element bodies and attributes.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render a daily digest email summarizing a batch of notifications.
/// Returns `None` when the input is empty — the caller should skip the
/// send rather than deliver an empty "here's what you missed" email.
///
/// `actor_names` maps actor_id → display name; missing entries fall back
/// to the raw id so a deleted actor row doesn't break the digest.
pub fn render_digest(
    notifs: &[Notification],
    actor_names: &std::collections::HashMap<String, String>,
    frontend_origin: &str,
    notif_secret: &[u8],
    exp_unix: i64,
) -> Option<RenderedEmail> {
    if notifs.is_empty() {
        return None;
    }

    let n = notifs.len();
    let subject = if n == 1 {
        "You have 1 unread update on OgreNotes".to_string()
    } else {
        format!("You have {n} unread updates on OgreNotes")
    };

    let mut text = format!(
        "You missed {n} update{plural} on OgreNotes:\n\n",
        plural = if n == 1 { "" } else { "s" },
    );
    let mut html_items = String::new();
    for notif in notifs {
        let actor = actor_names
            .get(&notif.actor_id)
            .cloned()
            .unwrap_or_else(|| notif.actor_id.clone());
        let event = event_label(&notif.notif_type);
        let link = link_for(notif, frontend_origin, notif_secret, exp_unix);
        text.push_str(&format!("• {actor} {event} — {}\n  {link}\n", notif.message));
        html_items.push_str(&format!(
            r#"<li><strong>{actor}</strong> {event} — {msg} <a href="{link}">open</a></li>"#,
            actor = html_escape(&actor),
            event = html_escape(event),
            msg = html_escape(&notif.message),
            link = html_escape(&link),
        ));
    }

    let html = format!(
        r#"<!doctype html><html><body style="font-family:sans-serif">
<h2>You missed {n} update{plural} on OgreNotes</h2>
<ul>{items}</ul>
<p style="color:#666;font-size:12px">You're receiving this digest because you've been away from OgreNotes for a while. Open any item to catch up.</p>
</body></html>"#,
        plural = if n == 1 { "" } else { "s" },
        items = html_items,
    );

    Some(RenderedEmail { subject, html, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test fixtures pin a deterministic secret + expiry so notif
    /// tokens are reproducible across test runs.
    const TEST_SECRET: &[u8] = b"test-notif-secret-32-bytes-long-12";
    const TEST_EXP: i64 = 2_000_000_000;

    fn sample_notif(t: NotifType, doc_id: Option<&str>) -> Notification {
        Notification {
            notif_id: "n1".into(),
            user_id: "recipient".into(),
            notif_type: t,
            doc_id: doc_id.map(String::from),
            thread_id: None,
            actor_id: "actor".into(),
            message: "alice replied to your comment".into(),
            preview: None,
            block_id: None,
            read: false,
            created_at: 0,
        }
    }

    #[test]
    fn includes_deep_link_to_doc() {
        let n = sample_notif(NotifType::Commented, Some("doc-42"));
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(r.html.contains("https://app.test/d/doc-42"), "html: {}", r.html);
        assert!(r.text.contains("https://app.test/d/doc-42"), "text: {}", r.text);
    }

    #[test]
    fn subject_carries_actor_and_event() {
        let n = sample_notif(NotifType::Shared, Some("doc-1"));
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(r.subject.starts_with("Alice "));
        assert!(r.subject.contains("shared"));
    }

    #[test]
    fn html_is_escaped() {
        let mut n = sample_notif(NotifType::Commented, Some("d"));
        n.message = "bad <script>".into();
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(!r.html.contains("<script>"), "script must be escaped: {}", r.html);
        assert!(r.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn falls_back_to_thread_link() {
        let mut n = sample_notif(NotifType::ChatMessage, None);
        n.thread_id = Some("t-9".into());
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(r.html.contains("/c/t-9"));
    }

    #[test]
    fn falls_back_to_origin_when_no_target() {
        let n = sample_notif(NotifType::DocumentEdited, None);
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(r.html.contains("https://app.test"));
    }

    #[test]
    fn every_notif_type_renders_without_panic() {
        for t in [
            NotifType::Shared,
            NotifType::Mentioned,
            NotifType::Commented,
            NotifType::ChatMessage,
            NotifType::DocumentEdited,
            NotifType::DocumentOpened,
            NotifType::RequestAccess,
        ] {
            let n = sample_notif(t, Some("d"));
            let _ = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        }
    }

    // ─── Digest ──────────────────────────────────────────────────

    #[test]
    fn digest_empty_returns_none() {
        // Don't send "here's what you missed (nothing)" mails.
        let actors = std::collections::HashMap::new();
        assert!(render_digest(&[], &actors, "https://app.test", TEST_SECRET, TEST_EXP).is_none());
    }

    #[test]
    fn digest_single_notif_renders_singular_subject() {
        let mut actors = std::collections::HashMap::new();
        actors.insert("actor".to_string(), "Alice".to_string());
        let notifs = vec![sample_notif(NotifType::Shared, Some("d1"))];
        let r = render_digest(&notifs, &actors, "https://app.test", TEST_SECRET, TEST_EXP).unwrap();
        assert!(r.subject.contains("1 unread"), "subject: {}", r.subject);
        assert!(!r.subject.contains("updates"), "should be singular: {}", r.subject);
        assert!(r.text.contains("Alice"));
        assert!(r.html.contains("https://app.test/d/d1"));
    }

    #[test]
    fn digest_many_notifs_renders_plural_and_lists_each() {
        let mut actors = std::collections::HashMap::new();
        actors.insert("actor".to_string(), "Alice".to_string());
        let mut notifs = Vec::new();
        for i in 0..5 {
            notifs.push(sample_notif(NotifType::Commented, Some(&format!("d{i}"))));
        }
        let r = render_digest(&notifs, &actors, "https://app.test", TEST_SECRET, TEST_EXP).unwrap();
        assert!(r.subject.contains("5 unread updates"));
        for i in 0..5 {
            let link = format!("https://app.test/d/d{i}");
            assert!(r.html.contains(&link), "expected {link} in {}", r.html);
        }
    }

    #[test]
    fn digest_escapes_html_in_actor_name_and_message() {
        let mut actors = std::collections::HashMap::new();
        actors.insert("actor".to_string(), "<script>Alice</script>".to_string());
        let mut notif = sample_notif(NotifType::Commented, Some("d"));
        notif.message = "hostile <img src=x>".to_string();
        let r = render_digest(&[notif], &actors, "https://app.test", TEST_SECRET, TEST_EXP).unwrap();
        assert!(!r.html.contains("<script>"), "html: {}", r.html);
        assert!(!r.html.contains("<img src=x>"), "html: {}", r.html);
        assert!(r.html.contains("&lt;script&gt;"));
    }

    #[test]
    fn digest_falls_back_to_actor_id_when_name_missing() {
        let actors = std::collections::HashMap::new(); // empty map
        let notifs = vec![sample_notif(NotifType::Shared, Some("d"))];
        let r = render_digest(&notifs, &actors, "https://app.test", TEST_SECRET, TEST_EXP).unwrap();
        // Raw actor_id should appear when the display-name lookup misses.
        assert!(r.text.contains("actor"), "text: {}", r.text);
    }

    // ─── #40: email-link HMAC token ──────────────────────────────

    #[test]
    fn link_includes_signed_notif_param() {
        let n = sample_notif(NotifType::Shared, Some("doc-99"));
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        // The link must carry `?notif=` followed by a signature + . + exp.
        let prefix = "https://app.test/d/doc-99?notif=";
        assert!(
            r.html.contains(prefix),
            "html missing signed link prefix: {}",
            r.html
        );
        assert!(
            r.html.contains(&format!(".{TEST_EXP}")),
            "html missing exp suffix: {}",
            r.html
        );
    }

    #[test]
    fn token_changes_with_target_id() {
        let a = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let b = build_notif_param("alice", "doc-2", TEST_EXP, TEST_SECRET);
        assert_ne!(a, b, "different doc_id must produce a different signature");
    }

    #[test]
    fn token_changes_with_user_id() {
        let a = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let b = build_notif_param("bob", "doc-1", TEST_EXP, TEST_SECRET);
        assert_ne!(a, b, "different user_id must produce a different signature");
    }

    #[test]
    fn token_changes_with_exp() {
        let a = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let b = build_notif_param("alice", "doc-1", TEST_EXP + 1, TEST_SECRET);
        assert_ne!(a, b, "different exp must produce a different signature");
    }

    #[test]
    fn token_changes_with_secret() {
        let a = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let b = build_notif_param("alice", "doc-1", TEST_EXP, b"different-secret");
        assert_ne!(a, b, "different secret must produce a different signature");
    }

    #[test]
    fn token_uses_url_safe_alphabet() {
        // Base64url with no padding — every char must be in
        // [A-Za-z0-9_-.=] (`.` is the exp delimiter; `=` ruled out
        // by NO_PAD). Critically: no `+`, `/`, `?`, `&` that would
        // break URL parsing.
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let value = param.strip_prefix("notif=").unwrap();
        for ch in value.chars() {
            assert!(
                ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.',
                "non-url-safe char {ch:?} in token {value}"
            );
        }
    }

    #[test]
    fn verify_accepts_freshly_built_token() {
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        let now = TEST_EXP - 1;
        assert!(verify_notif_param(raw, "alice", "doc-1", TEST_SECRET, now));
    }

    #[test]
    fn verify_rejects_expired_token() {
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        let now = TEST_EXP + 1; // past the exp
        assert!(!verify_notif_param(raw, "alice", "doc-1", TEST_SECRET, now));
    }

    #[test]
    fn verify_rejects_wrong_user_id() {
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        let now = TEST_EXP - 1;
        assert!(!verify_notif_param(raw, "mallory", "doc-1", TEST_SECRET, now));
    }

    #[test]
    fn verify_rejects_wrong_doc_id() {
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        let now = TEST_EXP - 1;
        assert!(!verify_notif_param(raw, "alice", "doc-evil", TEST_SECRET, now));
    }

    #[test]
    fn verify_rejects_tampered_signature() {
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        // Flip the first byte of the signature. The decoded byte
        // mismatch — the HMAC verify fails.
        let mut bytes: Vec<u8> = raw.bytes().collect();
        bytes[0] = if bytes[0] == b'A' { b'B' } else { b'A' };
        let tampered = std::str::from_utf8(&bytes).unwrap();
        let now = TEST_EXP - 1;
        assert!(!verify_notif_param(tampered, "alice", "doc-1", TEST_SECRET, now));
    }

    #[test]
    fn verify_rejects_lifted_exp() {
        // Attempt to extend the expiry by mutating the trailing
        // integer. The HMAC binds the exp, so any change is caught.
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        let (sig, _exp) = raw.split_once('.').unwrap();
        let lifted = format!("{sig}.{}", TEST_EXP + 10_000);
        let now = TEST_EXP - 1;
        assert!(!verify_notif_param(&lifted, "alice", "doc-1", TEST_SECRET, now));
    }

    #[test]
    fn verify_rejects_malformed_param() {
        // Missing dot, non-numeric exp, non-base64 signature —
        // every malformed shape returns false without panic.
        assert!(!verify_notif_param("nosignature", "u", "d", TEST_SECRET, 0));
        assert!(!verify_notif_param("aaaa.notnumeric", "u", "d", TEST_SECRET, 0));
        assert!(!verify_notif_param("!!!.123", "u", "d", TEST_SECRET, 0));
        assert!(!verify_notif_param("", "u", "d", TEST_SECRET, 0));
    }

    // ─── Escaping: actor name + full character set ───────────────

    #[test]
    fn render_escapes_actor_name_in_html() {
        // The actor's display name is user-controlled (profile field);
        // it must be escaped in the HTML body just like the message.
        // The existing `html_is_escaped` test only pins the message.
        let n = sample_notif(NotifType::Commented, Some("d"));
        let r = render(
            &n,
            r#"<img src=x onerror=alert(1)>Mallory"#,
            "https://app.test",
            TEST_SECRET,
            TEST_EXP,
        );
        assert!(
            !r.html.contains("<img"),
            "raw tag from actor name must not reach html: {}",
            r.html
        );
        assert!(
            r.html.contains("&lt;img src=x onerror=alert(1)&gt;Mallory"),
            "escaped actor name missing: {}",
            r.html
        );
    }

    #[test]
    fn render_escapes_ampersand_and_both_quote_kinds() {
        // `<`/`>` are pinned elsewhere; `&`, `"`, `'` matter for
        // attribute contexts (the link is spliced into an href) and
        // must each map to their entity exactly once (no double
        // escaping of the `&` inside an emitted entity).
        let mut n = sample_notif(NotifType::Commented, Some("d"));
        n.message = r#"Tom & Jerry said "hi" — it's fine"#.into();
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(
            r.html
                .contains(r#"Tom &amp; Jerry said &quot;hi&quot; — it&#39;s fine"#),
            "html: {}",
            r.html
        );
        assert!(
            !r.html.contains("&amp;amp;"),
            "ampersand must not be double-escaped: {}",
            r.html
        );
    }

    #[test]
    fn render_preserves_unicode_untouched() {
        // The escaper walks chars — multibyte content (CJK, emoji,
        // combining marks) must survive verbatim in subject, html,
        // and text.
        let mut n = sample_notif(NotifType::Mentioned, Some("d"));
        n.message = "日本語のコメント 🦀 café".into();
        let r = render(&n, "Ünïcødé Üser", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(r.subject.contains("Ünïcødé Üser"), "subject: {}", r.subject);
        assert!(r.html.contains("日本語のコメント 🦀 café"), "html: {}", r.html);
        assert!(r.text.contains("日本語のコメント 🦀 café"), "text: {}", r.text);
    }

    #[test]
    fn render_text_body_stays_plain_not_entity_encoded() {
        // The text/plain alternative must carry the raw message —
        // HTML entities leaking into the plain-text part would show
        // up literally in text-mode mail clients.
        let mut n = sample_notif(NotifType::Commented, Some("d"));
        n.message = r#"a < b && c > "d""#.into();
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(r.text.contains(r#"a < b && c > "d""#), "text: {}", r.text);
        assert!(!r.text.contains("&lt;"), "entities leaked into text: {}", r.text);
        assert!(!r.text.contains("&amp;"), "entities leaked into text: {}", r.text);
    }

    // ─── Link target selection ───────────────────────────────────

    #[test]
    fn doc_link_takes_precedence_over_thread_link() {
        // When a notification carries both a doc_id and a thread_id
        // (e.g. a comment in a doc-attached thread), the deep link
        // must point at the document.
        let mut n = sample_notif(NotifType::Commented, Some("doc-A"));
        n.thread_id = Some("thread-B".into());
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(r.html.contains("/d/doc-A"), "html: {}", r.html);
        assert!(!r.html.contains("/c/thread-B"), "html: {}", r.html);
        assert!(!r.text.contains("/c/thread-B"), "text: {}", r.text);
    }

    #[test]
    fn request_access_subject_names_the_event() {
        // RequestAccess is the owner-facing "someone wants edit
        // access" mail; the subject must say so.
        let n = sample_notif(NotifType::RequestAccess, Some("d"));
        let r = render(&n, "Alice", "https://app.test", TEST_SECRET, TEST_EXP);
        assert!(
            r.subject.contains("requested edit access"),
            "subject: {}",
            r.subject
        );
    }

    // ─── Digest: signed links + multi-actor resolution ───────────

    #[test]
    fn digest_links_carry_signed_notif_param() {
        // Digest deep links go through the same signed-token path as
        // per-event mails (#40); only the per-event side was pinned.
        let mut actors = std::collections::HashMap::new();
        actors.insert("actor".to_string(), "Alice".to_string());
        let notifs = vec![sample_notif(NotifType::Shared, Some("d1"))];
        let r = render_digest(&notifs, &actors, "https://app.test", TEST_SECRET, TEST_EXP).unwrap();
        assert!(
            r.html.contains("https://app.test/d/d1?notif="),
            "digest link missing signed param: {}",
            r.html
        );
        assert!(
            r.html.contains(&format!(".{TEST_EXP}")),
            "digest link missing exp suffix: {}",
            r.html
        );
    }

    #[test]
    fn digest_resolves_each_distinct_actor_name() {
        // Two notifications from two different actors: each display
        // name must be looked up independently, not smeared from the
        // first entry.
        let mut actors = std::collections::HashMap::new();
        actors.insert("actor".to_string(), "Alice".to_string());
        actors.insert("actor-2".to_string(), "Bob".to_string());
        let first = sample_notif(NotifType::Shared, Some("d1"));
        let mut second = sample_notif(NotifType::Commented, Some("d2"));
        second.actor_id = "actor-2".into();
        let r = render_digest(
            &[first, second],
            &actors,
            "https://app.test",
            TEST_SECRET,
            TEST_EXP,
        )
        .unwrap();
        assert!(r.html.contains("Alice"), "html: {}", r.html);
        assert!(r.html.contains("Bob"), "html: {}", r.html);
        assert!(r.text.contains("Alice"), "text: {}", r.text);
        assert!(r.text.contains("Bob"), "text: {}", r.text);
    }

    // ─── #40 token: additional verify edges ──────────────────────

    #[test]
    fn build_notif_param_is_deterministic() {
        // Same (user, target, exp, secret) must produce the same
        // token — resends and retries yield identical URLs.
        let a = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let b = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        assert_eq!(a, b);
    }

    #[test]
    fn verify_rejects_token_at_exact_expiry_instant() {
        // Expiry is strict: `exp > now`. A token checked at exactly
        // its expiry second is already dead.
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        assert!(!verify_notif_param(raw, "alice", "doc-1", TEST_SECRET, TEST_EXP));
    }

    #[test]
    fn verify_rejects_wrong_secret() {
        // A token minted under one workspace secret must not verify
        // under another — key rotation invalidates old links.
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        let now = TEST_EXP - 1;
        assert!(!verify_notif_param(raw, "alice", "doc-1", b"rotated-secret", now));
    }

    #[test]
    fn verify_rejects_param_with_extra_dot_segment() {
        // `split_once('.')` binds at the first dot, so a trailing
        // ".999" corrupts the exp segment ("<exp>.999" fails to
        // parse) rather than being silently ignored.
        let param = build_notif_param("alice", "doc-1", TEST_EXP, TEST_SECRET);
        let raw = param.strip_prefix("notif=").unwrap();
        let extended = format!("{raw}.999");
        let now = TEST_EXP - 1;
        assert!(!verify_notif_param(&extended, "alice", "doc-1", TEST_SECRET, now));
    }
}
