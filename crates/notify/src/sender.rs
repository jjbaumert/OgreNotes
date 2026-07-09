// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! SMTP + no-op email transports.
//!
//! `EmailSender` is the abstraction every caller depends on so tests and
//! environments with `email_enabled=false` can swap in `NoopSender` without
//! touching the service wiring.

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SendError {
    #[error("invalid email address: {0}")]
    InvalidAddress(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("build error: {0}")]
    Build(String),
    /// Cap-counter backing store failed. Distinguished from `Transport`
    /// so oncall doesn't chase SMTP when the real issue is DynamoDB.
    #[error("cap store error: {0}")]
    Cap(String),
}

#[async_trait]
pub trait EmailSender: Send + Sync {
    /// Send an email. `html` and `text` are both provided; senders that
    /// support multipart attach both, senders that don't use `text`.
    async fn send(
        &self,
        from: &str,
        to: &str,
        subject: &str,
        html: &str,
        text: &str,
    ) -> Result<(), SendError>;
}

/// No-op sender used when `email_enabled=false` and in unit tests. Every
/// call returns `Ok(())` without touching the network.
pub struct NoopSender;

#[async_trait]
impl EmailSender for NoopSender {
    async fn send(
        &self,
        _from: &str,
        _to: &str,
        _subject: &str,
        _html: &str,
        _text: &str,
    ) -> Result<(), SendError> {
        Ok(())
    }
}

/// SMTP sender backed by lettre. Connects to MailHog locally or the SES
/// SMTP relay in prod — the difference is config, not code.
pub struct SmtpSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl SmtpSender {
    /// Build an SMTP sender from host/port/credentials. `starttls=false` is
    /// the right choice for local dev (MailHog does not speak TLS); prod
    /// should always pass `true`.
    pub fn new(
        host: &str,
        port: u16,
        username: Option<&str>,
        password: Option<&str>,
        starttls: bool,
    ) -> Result<Self, SendError> {
        let mut builder = if starttls {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
                .map_err(|e| SendError::Build(e.to_string()))?
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        };
        builder = builder.port(port);
        if let (Some(u), Some(p)) = (username, password) {
            builder = builder.credentials(Credentials::new(u.to_string(), p.to_string()));
        }
        Ok(Self {
            transport: builder.build(),
        })
    }
}

#[async_trait]
impl EmailSender for SmtpSender {
    async fn send(
        &self,
        from: &str,
        to: &str,
        subject: &str,
        html: &str,
        text: &str,
    ) -> Result<(), SendError> {
        let from_mbox = Mailbox::from_str(from)
            .map_err(|e| SendError::InvalidAddress(format!("from {from}: {e}")))?;
        let to_mbox = Mailbox::from_str(to)
            .map_err(|e| SendError::InvalidAddress(format!("to {to}: {e}")))?;

        let msg = Message::builder()
            .from(from_mbox)
            .to(to_mbox)
            .subject(subject)
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(text.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(html.to_string()),
                    ),
            )
            .map_err(|e| SendError::Build(e.to_string()))?;

        self.transport
            .send(msg)
            .await
            .map_err(|e| SendError::Transport(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_sender_is_always_ok() {
        let s = NoopSender;
        assert!(s.send("a@b.com", "c@d.com", "s", "h", "t").await.is_ok());
    }

    /// A malformed `from`/`to` address must be rejected at parse time —
    /// before any SMTP connection is attempted — so a bad address surfaces
    /// as `InvalidAddress`, not a `Transport` timeout. We point the sender
    /// at an unroutable host (`builder_dangerous` does not dial on build);
    /// the address guard returns first, so the host is never contacted.
    fn unroutable_sender() -> SmtpSender {
        // 192.0.2.0/24 is TEST-NET-1 (RFC 5737) — guaranteed never routed.
        SmtpSender::new("192.0.2.1", 2525, None, None, false).expect("build")
    }

    #[tokio::test]
    async fn smtp_rejects_invalid_from_before_dialing() {
        let s = unroutable_sender();
        let res = s
            .send("not a valid address", "c@d.com", "s", "h", "t")
            .await;
        assert!(
            matches!(res, Err(SendError::InvalidAddress(ref m)) if m.starts_with("from ")),
            "expected InvalidAddress(from …), got {res:?}"
        );
    }

    #[tokio::test]
    async fn smtp_rejects_invalid_to_before_dialing() {
        let s = unroutable_sender();
        // Valid from, malformed to → the guard must name the `to` side.
        let res = s.send("a@b.com", "@@nope@@", "s", "h", "t").await;
        assert!(
            matches!(res, Err(SendError::InvalidAddress(ref m)) if m.starts_with("to ")),
            "expected InvalidAddress(to …), got {res:?}"
        );
    }

    #[test]
    fn send_error_display_distinguishes_variants() {
        // The variant prefixes are an oncall contract: `Cap` exists
        // specifically so a DynamoDB cap-store failure isn't chased
        // as an SMTP problem. Pin each Display rendering.
        assert_eq!(
            SendError::Cap("dynamo down".into()).to_string(),
            "cap store error: dynamo down"
        );
        assert_eq!(
            SendError::Transport("refused".into()).to_string(),
            "transport error: refused"
        );
        assert_eq!(
            SendError::InvalidAddress("from x: bad".into()).to_string(),
            "invalid email address: from x: bad"
        );
        assert_eq!(
            SendError::Build("missing header".into()).to_string(),
            "build error: missing header"
        );
    }
}
