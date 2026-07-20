//! Provider-neutral bibliography vocabulary shared by Cita crates.

mod locator;
mod reference;

pub use locator::{Error, Locator, normalize_arxiv, normalize_doi, strip_arxiv_version};
pub use reference::{
    Identifiers, MetadataProvider, ProjectionError, ProviderError, Reference, ReferenceSource,
};
