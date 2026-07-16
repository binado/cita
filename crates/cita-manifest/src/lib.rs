//! Manifest storage and deterministic BibTeX export for Cita.
//!
//! Owns the `cita.toml` on-disk format: comment-preserving edits via
//! `toml_edit`, atomic saves, identity-based de-duplication, and BibTeX
//! rendering. Provider-neutral vocabulary lives in `cita-core`.

mod bibtex;
mod manifest;

pub use bibtex::export_bibtex;
pub use manifest::{AddOutcome, Error, Manifest};
