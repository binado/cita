//! Strict, byte-preserving BibTeX syntax handling.
//!
//! This crate is the complete boundary around BibTeX syntax in bibi. It scans
//! entry and citation-key spans, validates standalone entries, re-keys them
//! without touching any other byte, extracts identifier candidates, projects
//! local metadata, and renders a deterministic sequence of entries.
//!
//! Two rules shape the API.
//!
//! **Stored BibTeX is byte-identical to what it was given.** The only permitted
//! transformation is rewriting the citation-key token, which is why
//! [`BibtexEntry`] is built around spans rather than a parsed representation,
//! and why there is no writer that could re-emit an entry from its fields.
//!
//! **Structure and meaning are read separately.** [`BibtexEntry::parse_one`],
//! [`parse_file`], and [`BibtexEntry::identifier_candidates`] are purely
//! syntactic and require no title. [`BibtexEntry::local_metadata`] is the one
//! semantic read, and it is reserved for entries the user supplied, where the
//! BibTeX is the original rather than a rendering of a structured record.
#![warn(missing_docs)]

mod entry;
mod error;
mod file;
mod local_metadata;
mod render;
mod scanner;

pub use entry::{BibtexEntry, CitationKey, IdentifierCandidates};
pub use error::Error;
pub use file::{ImportedEntry, parse_file};
pub use local_metadata::LocalMetadata;
pub use render::render;
