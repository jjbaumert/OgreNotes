// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! SCIM 2.0 (RFC 7643/7644) protocol surface (Phase 4 M-E5).
//!
//! `dtos` contains the wire-shape structs used by piece D's
//! `/Users` and piece E's `/Groups` + static endpoints. The DTOs
//! deliberately do not include any DDB / storage layer concerns —
//! they are pure JSON shapes per the RFC, and the route handlers
//! marshal between these and the internal `User` / workspace-member
//! domain types.

pub mod discovery;
pub mod dtos;
pub mod filter;
pub mod mapping;
