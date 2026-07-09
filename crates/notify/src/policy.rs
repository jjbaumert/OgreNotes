// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Decision functions for whether a notification becomes an email.
//!
//! Kept pure (no I/O, no async) so the matrix of
//! `(pref, notif_type, is_direct)` combinations is exhaustively testable.

use ogrenotes_storage::models::notification::NotifType;
use ogrenotes_storage::models::NotifEmailPref;

/// How long after a user's last authenticated request we still consider
/// them "in-app" and skip sending email for their notifications. Matches
/// the debounce window on the `ActivityTracker` writer so the semantics
/// of "active" are consistent across the two call sites.
pub const ACTIVE_WINDOW_USEC: i64 = 5 * 60 * 1_000_000;

/// Does the user's email pref allow this notification type to be emailed?
///
/// `is_direct=true` means the event is directly addressed at the recipient
/// — a reply to their comment, an @mention, a share invite targeted at
/// them. Under `MentionsOnly` (the default), only direct events and
/// explicit `Mentioned`/`Shared` types are emailed.
pub fn should_email_for_prefs(
    pref: NotifEmailPref,
    notif_type: &NotifType,
    is_direct: bool,
) -> bool {
    match pref {
        NotifEmailPref::Disabled => false,
        NotifEmailPref::All => true,
        NotifEmailPref::MentionsOnly => {
            is_direct || matches!(notif_type, NotifType::Mentioned | NotifType::Shared)
        }
    }
}

/// Is the user currently active in-app? Used to suppress email when the
/// recipient already has the app open and will see the in-app
/// notification immediately.
///
/// `last_active_at == 0` means "we have never recorded activity" — treat
/// as not-active so the first notification after signup still sends.
pub fn is_recently_active(last_active_at: i64, now: i64) -> bool {
    last_active_at > 0 && now - last_active_at < ACTIVE_WINDOW_USEC
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_never_emails() {
        for is_direct in [true, false] {
            for t in [
                NotifType::Shared,
                NotifType::Mentioned,
                NotifType::Commented,
                NotifType::ChatMessage,
                NotifType::DocumentEdited,
                NotifType::DocumentOpened,
            ] {
                assert!(!should_email_for_prefs(
                    NotifEmailPref::Disabled,
                    &t,
                    is_direct
                ));
            }
        }
    }

    #[test]
    fn all_always_emails() {
        for is_direct in [true, false] {
            for t in [
                NotifType::Shared,
                NotifType::Mentioned,
                NotifType::Commented,
                NotifType::ChatMessage,
                NotifType::DocumentEdited,
                NotifType::DocumentOpened,
            ] {
                assert!(should_email_for_prefs(NotifEmailPref::All, &t, is_direct));
            }
        }
    }

    #[test]
    fn mentions_only_emails_explicit_mention_and_share() {
        assert!(should_email_for_prefs(
            NotifEmailPref::MentionsOnly,
            &NotifType::Mentioned,
            false
        ));
        assert!(should_email_for_prefs(
            NotifEmailPref::MentionsOnly,
            &NotifType::Shared,
            false
        ));
    }

    #[test]
    fn mentions_only_emails_direct_reply() {
        assert!(should_email_for_prefs(
            NotifEmailPref::MentionsOnly,
            &NotifType::Commented,
            true
        ));
    }

    #[test]
    fn mentions_only_skips_indirect() {
        // Someone commented on a doc you own but didn't reply to you.
        assert!(!should_email_for_prefs(
            NotifEmailPref::MentionsOnly,
            &NotifType::Commented,
            false
        ));
        // Chat messages are not considered mentions.
        assert!(!should_email_for_prefs(
            NotifEmailPref::MentionsOnly,
            &NotifType::ChatMessage,
            false
        ));
        // Document opens are not mentions.
        assert!(!should_email_for_prefs(
            NotifEmailPref::MentionsOnly,
            &NotifType::DocumentOpened,
            false
        ));
    }

    #[test]
    fn zero_last_active_is_not_active() {
        // Brand-new users have last_active_at = 0; do not treat as active.
        assert!(!is_recently_active(0, 1_000_000_000));
    }

    #[test]
    fn active_within_window() {
        let now: i64 = 10_000_000_000;
        assert!(is_recently_active(now - 1, now));
        assert!(is_recently_active(now - ACTIVE_WINDOW_USEC + 1, now));
    }

    #[test]
    fn not_active_past_window() {
        let now: i64 = 10_000_000_000;
        assert!(!is_recently_active(now - ACTIVE_WINDOW_USEC, now));
        assert!(!is_recently_active(now - ACTIVE_WINDOW_USEC - 1, now));
    }

    #[test]
    fn request_access_pref_matrix() {
        // RequestAccess postdates the loops above and is absent from
        // them. Pin its full pref matrix: it is NOT special-cased by
        // MentionsOnly (unlike Mentioned/Shared), so delivery under
        // the default pref relies on the call site passing
        // is_direct=true — which the route does, because an edit-
        // access request is always aimed at the document owner.
        let t = NotifType::RequestAccess;
        for is_direct in [true, false] {
            assert!(!should_email_for_prefs(NotifEmailPref::Disabled, &t, is_direct));
            assert!(should_email_for_prefs(NotifEmailPref::All, &t, is_direct));
        }
        assert!(should_email_for_prefs(NotifEmailPref::MentionsOnly, &t, true));
        assert!(!should_email_for_prefs(NotifEmailPref::MentionsOnly, &t, false));
    }

    #[test]
    fn negative_last_active_is_not_active() {
        // Corrupt / sentinel negative timestamps must read as
        // never-active, same as the 0 sentinel — the guard is
        // `last_active_at > 0`, not merely "recent".
        assert!(!is_recently_active(-1, 1_000_000_000));
        assert!(!is_recently_active(i64::MIN, 1_000_000_000));
    }
}
