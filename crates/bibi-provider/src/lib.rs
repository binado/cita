//! The provider contract, provider-neutral results, registry, and local provider.
//!
//! Providers differ in capability, not in kind. This crate defines the
//! object-safe contract every provider implements, the ordered registry that
//! dispatches to them, and the local provider that ingests user-supplied
//! BibTeX and never refreshes.
#![warn(missing_docs)]
