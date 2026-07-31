//! Normalized, operational identifiers.
//!
//! Identifiers are not description. A wrong title prints a poor listing; a
//! wrong identifier deduplicates against the wrong record, resolves a selector
//! to the wrong record, or fetches the wrong document. They are also immutable
//! in a way description is not: an arXiv identifier never changes, and a DOI may
//! be *added* on publication but never changes value.

mod arxiv;
mod doi;

pub use arxiv::ArxivId;
pub use doi::Doi;
