//! Schema-1 `bibi.toml` persistence.
//!
//! This crate owns the serialized manifest shape, candidate validation and its
//! rebuilt indexes, generation-based optimistic concurrency, and the atomic
//! commit sequence. It knows nothing of providers, HTTP, or commands — the
//! coupling that let the previous tool's manifest layer name one specific
//! metadata API is not representable here.
//!
//! The manifest is the only authoritative project state and the only file bibi
//! writes into a project directory. There is no lock and no sidecar: atomic
//! replacement means a reader sees either the whole previous file or the whole
//! next one, and a generation comparison catches the stale-write case that
//! actually occurs in practice.
#![warn(missing_docs)]

mod atomic;
mod candidate;
mod error;
mod indexes;
mod mutation;
mod schema;
mod store;
#[cfg(test)]
mod test_support;

pub use atomic::atomic_replace;
pub use candidate::{Manifest, ManifestCandidate};
pub use error::Error;
pub use schema::SCHEMA;
pub use store::{Generation, LoadedManifest, ManifestStore};
