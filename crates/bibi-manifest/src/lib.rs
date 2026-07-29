//! Schema-1 `bibi.toml` persistence.
//!
//! This crate owns the serialized manifest shape, candidate validation and its
//! rebuilt indexes, generation-based optimistic concurrency, and the atomic
//! commit sequence. It knows nothing of providers, HTTP, or commands.
#![warn(missing_docs)]
