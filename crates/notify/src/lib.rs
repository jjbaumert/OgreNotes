// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Email notifications for OgreNotes (M4).
//!
//! `EmailService` is the façade every route handler hits via
//! `spawn_for_notification` — a one-liner that fires email delivery in the
//! background. The send path consults (in order) the `email_enabled`
//! config flag, the user's email address + `NotifEmailPref`, the "active
//! in-app" suppression window, and the per-user daily cap.
//!
//! Transport: `lettre` SMTP. The same binary talks to MailHog locally
//! (port 1025, no TLS) and to SES SMTP in production
//! (`email-smtp.<region>.amazonaws.com:587`, STARTTLS). No `aws-sdk-ses`
//! dependency — SES SMTP credentials are plain username/password.

pub mod cap;
pub mod policy;
pub mod sender;
pub mod service;
pub mod templates;

pub use cap::EmailCapRepo;
pub use policy::{is_recently_active, should_email_for_prefs, ACTIVE_WINDOW_USEC};
pub use sender::{EmailSender, NoopSender, SendError, SmtpSender};
pub use service::{EmailService, SendOutcome};
