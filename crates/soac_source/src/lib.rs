//! Source features and token restrictions shared by lowering and offline analysis.
//!
//! This is a fail-closed boundary for a representation that cannot preserve
//! every Python string. It does not change native Python or claim to make Ruff's
//! decoded string payload lossless.

#![deny(unreachable_pub)]

mod futures;
mod literals;

pub use futures::has_strict_future;
pub use literals::{UnsupportedSurrogateEscape, validate_source_literals};
