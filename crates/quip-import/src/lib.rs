//! Quip import — client, throttle, token store, converter (design:
//! docs/superpowers/specs/2026-07-24-document-mentions… quip-import-design.md).
pub mod secret;
pub mod throttle;
pub use secret::QuipToken;
pub use throttle::{RateState, Throttle};
