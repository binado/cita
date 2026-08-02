//! Strict, byte-preserving BibTeX syntax handling.
//!
//! This crate is the complete boundary around BibTeX syntax in bibi. It scans
//! entries and texkeys, validates standalone entries, projects local metadata,
//! and renders a deterministic sequence of entries.
//!
//! Two rules shape the API.
//!
//! **Stored BibTeX is byte-identical to what it was given.** The only permitted
//! transformation is permitted, which is why there is no writer that could
//! re-emit an entry from its fields.
//!
//! **Structure and meaning are read separately.** [`parse_file`] is purely
//! structural and requires no title. [`project_local`] is the semantic read,
//! and it is reserved for entries the user supplied, where the BibTeX is the
//! original rather than a rendering of a structured record.
#![warn(missing_docs)]

mod adapter;
mod entry;
mod error;
mod file;
mod local_metadata;
mod render;
mod scanner;

pub use error::Error;
pub use file::parse_file;
pub use render::render;

/// Parse one local entry and project the metadata needed by the domain.
pub fn project_local(
    source: &str,
) -> Result<
    (
        bibi_core::Bibtex,
        bibi_core::Identifiers,
        bibi_core::Description,
    ),
    Error,
> {
    let entry = entry::BibtexEntry::parse_one(source.to_owned())?;
    let candidates = entry.identifier_candidates();
    let metadata = entry.local_metadata()?;
    let identifiers = bibi_core::Identifiers::new(
        candidates
            .doi
            .as_deref()
            .and_then(|value| bibi_core::Doi::new(value).ok()),
        candidates
            .arxiv
            .as_deref()
            .and_then(|value| bibi_core::ArxivId::new(value).ok()),
    );
    let bibtex = entry.into_core();
    let description = bibi_core::Description::new(
        metadata.title,
        metadata.authors,
        metadata.collaborations,
        metadata.year,
    );
    Ok((bibtex, identifiers, description))
}
