//! Schema-1 `bibi.toml` persistence for a core [`bibi_core::Bibliography`].
//!
//! Domain validation belongs to `bibi-core`. This crate only converts the
//! strict wire shape, detects stale generations, and atomically publishes one
//! complete successor bibliography.
#![warn(missing_docs)]

mod atomic;
mod error;
mod schema;
mod store;

pub use atomic::atomic_replace;
pub use error::Error;
pub use schema::SCHEMA;
pub use store::{BibliographyStore, Generation, LoadedBibliography};
