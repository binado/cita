//! Provider-neutral bibliography vocabulary shared by bibi crates.
#![warn(missing_docs)]

mod locator;
mod reference;

pub use locator::{Error, Locator, normalize_arxiv, normalize_doi, strip_arxiv_version};
pub use reference::{
    Identifiers, MetadataProvider, ProjectionError, ProviderError, Reference, ReferenceSource,
};
