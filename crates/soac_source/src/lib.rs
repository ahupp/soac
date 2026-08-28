//! Source features, policy comments and token restrictions shared by lowering
//! and offline analysis.
//!
//! Policy comments retain original source and class ranges without resolving
//! inheritance or granting runtime authority. Literal validation is a
//! fail-closed boundary for a representation that cannot preserve every Python
//! string; it does not change native Python or make Ruff's decoded payload
//! lossless.

#![deny(unreachable_pub)]

mod directives;
mod futures;
mod literals;

pub use directives::{
    SoacDirective, SoacDirectiveError, SoacDirectiveErrorKind, SoacDirectiveTarget,
    parse_soac_directives,
};
pub use futures::has_strict_future;
pub use literals::{UnsupportedSurrogateEscape, validate_source_literals};
