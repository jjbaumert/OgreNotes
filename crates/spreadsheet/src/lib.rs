// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Spreadsheet evaluation shared by the browser and server-side exports.
//!
//! Browser date and random functions retain their JavaScript behavior. Native
//! evaluation uses UTC for dates and accepts ISO dates, ISO local timestamps,
//! and RFC 3339 timestamps. Browser-specific date strings are not portable.
//! Volatile formulas (TODAY, NOW, RAND, and their dependents) are evaluated at
//! export time and can differ from the browser's last displayed result.

pub mod embedded;
pub mod eval;
pub mod functions;
pub mod parser;
pub mod pivot;
pub mod stats;
pub mod translate;

mod runtime;
