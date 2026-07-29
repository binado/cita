//! Retrieving an arXiv artifact.
//!
//! This crate acquires one file: it downloads a work's PDF or original source
//! archive to a path the caller names, validates that the response really is
//! that kind of file, and publishes it through a temporary sibling so a
//! destination never holds a partial download.
//!
//! It stores nothing. There is no cache, no root it owns, no layout it
//! maintains, and no eviction to schedule — which is why nothing here has to
//! answer what becomes of a downloaded file when a record is renamed or
//! removed. Managing a document collection is a library concern rather than a
//! project one, and a project tool that acquires files does not need to become
//! one to be useful.
#![warn(missing_docs)]

mod error;
mod naming;
mod store;

pub use error::Error;
pub use naming::{ArtifactKind, default_filename};
pub use store::{ArtifactClient, ArtifactClientBuilder, public_url};
