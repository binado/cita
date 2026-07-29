//! Strict, byte-preserving BibTeX syntax handling.
//!
//! This crate is the complete boundary around BibTeX syntax in bibi: it scans
//! entry and citation-key spans, validates standalone entries, re-keys them
//! without touching any other byte, extracts identifier candidates, projects
//! local metadata, and renders a deterministic sequence of entries.
#![warn(missing_docs)]
