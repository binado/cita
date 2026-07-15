//! Manifest storage and deterministic BibTeX export for PaperDB.
//!
//! Owns the `paperdb.toml` on-disk format: comment-preserving edits via
//! `toml_edit`, atomic saves, identity-based de-duplication, and BibTeX
//! rendering. Provider-neutral vocabulary lives in `paperdb-core`.

mod bibtex;
mod manifest;

pub use bibtex::export_bibtex;
pub use manifest::{AddOutcome, Error, Manifest};
