// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Slash-command parsing for chat / comment messages.
//!
//! Commands are detected *before* a message is persisted. A match means the
//! originally-typed text is replaced with a system-styled announcement (so
//! the stored message explains what happened, not the raw command string).
//! No match → the message flows through unchanged.

/// Built-in kaomoji decorations. These avoid taking a third-party
/// dependency (e.g. Giphy) for "fun" message ornaments — the glyph is
/// embedded in the binary and nothing leaves the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KaomojiKind {
    Shrug,
    TableFlip,
    UnFlip,
}

impl KaomojiKind {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Shrug => "¯\\_(ツ)_/¯",
            Self::TableFlip => "(╯°□°）╯︵ ┻━┻",
            Self::UnFlip => "┬─┬ ノ( ゜-゜ノ)",
        }
    }
}

/// One recognized command. Borrowed from the input string so parsing is
/// allocation-free; callers that need an owned form copy the fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand<'a> {
    /// `/invite @handle` — handle may be a bare id, an email, or an
    /// email/id prefixed with `@`. Leading `@` is stripped.
    Invite { handle: &'a str },
    /// `/shrug`, `/tableflip`, `/unflip` — appends a kaomoji to any
    /// preceding text. `prefix` is the trimmed text the user typed
    /// alongside the command (may be empty).
    Kaomoji { kind: KaomojiKind, prefix: &'a str },
    /// `/me <action>` — emote-style line. Action is required.
    Me { action: &'a str },
}

/// Try to parse `content` as a slash command. Returns `None` if the text is
/// not a recognized command (so the caller treats it as a normal message).
///
/// Whitespace on either end of the message is ignored; a trailing argument
/// is returned trimmed. An unknown `/…` word returns `None` rather than an
/// error, so users typing `/` in the middle of a sentence (e.g., "TODO: /
/// fix this") are not blocked.
pub fn try_parse(content: &str) -> Option<SlashCommand<'_>> {
    let trimmed = content.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let (cmd, rest) = trimmed
        .split_once(char::is_whitespace)
        .map(|(c, r)| (c, r.trim()))
        .unwrap_or((trimmed, ""));
    match cmd {
        "/invite" => {
            let handle = rest.trim_start_matches('@');
            if handle.is_empty() {
                None
            } else {
                Some(SlashCommand::Invite { handle })
            }
        }
        "/shrug" => Some(SlashCommand::Kaomoji { kind: KaomojiKind::Shrug, prefix: rest }),
        "/tableflip" => Some(SlashCommand::Kaomoji { kind: KaomojiKind::TableFlip, prefix: rest }),
        "/unflip" => Some(SlashCommand::Kaomoji { kind: KaomojiKind::UnFlip, prefix: rest }),
        "/me" => {
            if rest.is_empty() {
                None
            } else {
                Some(SlashCommand::Me { action: rest })
            }
        }
        _ => None,
    }
}

use ogrenotes_storage::models::thread::{MessagePart, PartStyle};
use ogrenotes_storage::models::user::User;
use ogrenotes_storage::repo::user_repo::UserRepo;

use crate::error::ApiError;

/// Look up an invite target by email (when the handle looks like one), then
/// fall back to treating it as a user_id. Returns a 404 ApiError if neither
/// lookup matches.
pub async fn resolve_handle(
    user_repo: &UserRepo,
    handle: &str,
) -> Result<User, ApiError> {
    if handle.contains('@') {
        if let Ok(Some(u)) = user_repo.get_by_email(handle).await {
            return Ok(u);
        }
    }
    match user_repo.get_by_id(handle).await {
        Ok(Some(u)) => Ok(u),
        Ok(None) => Err(ApiError::NotFound(format!("User '{handle}' not found"))),
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

/// Build the stored announcement for a successful `/invite`. The returned
/// tuple is `(content, parts)`: `content` is the plain-text form that
/// existing clients read; `parts` carries the System style so rich clients
/// can render the announcement distinctly from regular chat lines.
pub fn invite_announcement(actor_name: &str, invitee_display: &str) -> (String, Vec<MessagePart>) {
    let text = format!("{actor_name} invited {invitee_display}");
    let parts = vec![MessagePart {
        style: PartStyle::System,
        text: text.clone(),
    }];
    (text, parts)
}

/// Build the stored message for a `/shrug`, `/tableflip`, or `/unflip`
/// command: the user's prefix text (if any) followed by the kaomoji.
/// Renders as a normal message body so the kaomoji appears as the user's
/// own line — matching how Slack handles the same set.
pub fn render_kaomoji(kind: KaomojiKind, prefix: &str) -> (String, Vec<MessagePart>) {
    let prefix = prefix.trim();
    let text = if prefix.is_empty() {
        kind.glyph().to_string()
    } else {
        format!("{prefix} {}", kind.glyph())
    };
    let parts = vec![MessagePart {
        style: PartStyle::Body,
        text: text.clone(),
    }];
    (text, parts)
}

/// Build the stored announcement for a `/me <action>` command. Rendered
/// with `PartStyle::System` so rich clients style it like an emote
/// (`Alice waves hello`) rather than a normal chat line.
pub fn me_announcement(actor_name: &str, action: &str) -> (String, Vec<MessagePart>) {
    let text = format!("{actor_name} {}", action.trim());
    let parts = vec![MessagePart {
        style: PartStyle::System,
        text: text.clone(),
    }];
    (text, parts)
}

/// Resolve the caller's display name from the user repo, falling back to
/// the raw user_id when the lookup fails so messages still read sanely
/// even if the user record is missing.
pub async fn resolve_actor_name(user_repo: &UserRepo, user_id: &str) -> String {
    match user_repo.get_by_id(user_id).await {
        Ok(Some(u)) => u.name,
        _ => user_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_invite_with_at() {
        assert_eq!(
            try_parse("/invite @bob@test.com"),
            Some(SlashCommand::Invite { handle: "bob@test.com" })
        );
    }

    #[test]
    fn parses_invite_without_at() {
        assert_eq!(
            try_parse("/invite bob@test.com"),
            Some(SlashCommand::Invite { handle: "bob@test.com" })
        );
    }

    #[test]
    fn invite_requires_handle() {
        assert_eq!(try_parse("/invite"), None);
        assert_eq!(try_parse("/invite   "), None);
        assert_eq!(try_parse("/invite @"), None);
    }

    #[test]
    fn parses_shrug_with_text() {
        assert_eq!(
            try_parse("/shrug who knows"),
            Some(SlashCommand::Kaomoji { kind: KaomojiKind::Shrug, prefix: "who knows" })
        );
    }

    #[test]
    fn parses_shrug_alone() {
        assert_eq!(
            try_parse("/shrug"),
            Some(SlashCommand::Kaomoji { kind: KaomojiKind::Shrug, prefix: "" })
        );
    }

    #[test]
    fn parses_tableflip_and_unflip() {
        assert!(matches!(
            try_parse("/tableflip"),
            Some(SlashCommand::Kaomoji { kind: KaomojiKind::TableFlip, .. })
        ));
        assert!(matches!(
            try_parse("/unflip calmly"),
            Some(SlashCommand::Kaomoji { kind: KaomojiKind::UnFlip, prefix: "calmly" })
        ));
    }

    #[test]
    fn parses_me_with_action() {
        assert_eq!(
            try_parse("/me waves hello"),
            Some(SlashCommand::Me { action: "waves hello" })
        );
    }

    #[test]
    fn me_requires_action() {
        assert_eq!(try_parse("/me"), None);
        assert_eq!(try_parse("/me   "), None);
    }

    #[test]
    fn render_kaomoji_appends_glyph() {
        let (text, parts) = render_kaomoji(KaomojiKind::Shrug, "who knows");
        assert_eq!(text, "who knows ¯\\_(ツ)_/¯");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].style, PartStyle::Body);
    }

    #[test]
    fn render_kaomoji_alone_emits_just_glyph() {
        let (text, _parts) = render_kaomoji(KaomojiKind::TableFlip, "");
        assert_eq!(text, "(╯°□°）╯︵ ┻━┻");
    }

    #[test]
    fn me_announcement_uses_system_style() {
        let (text, parts) = me_announcement("Alice", "waves hello");
        assert_eq!(text, "Alice waves hello");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].style, PartStyle::System);
    }

    #[test]
    fn unknown_command_is_not_parsed() {
        // Unknown slashes pass through as normal messages.
        assert_eq!(try_parse("/helpme"), None);
        assert_eq!(try_parse("/giphy anything"), None);
    }

    #[test]
    fn non_slash_returns_none() {
        assert_eq!(try_parse("hello world"), None);
        assert_eq!(try_parse(""), None);
    }

    #[test]
    fn ignores_leading_whitespace() {
        assert_eq!(
            try_parse("   /invite alice"),
            Some(SlashCommand::Invite { handle: "alice" })
        );
    }
}
