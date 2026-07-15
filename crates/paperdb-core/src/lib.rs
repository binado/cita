//! Provider-neutral types, manifest management, citation keys, and BibTeX.

mod bibtex;
mod key;
mod locator;
mod manifest;
mod model;
mod provider;

pub use bibtex::export_bibtex;
pub use key::{fallback_key, validate_key};
pub use locator::{Locator, strip_arxiv_version};
pub use manifest::{AddOutcome, Manifest, EXPLICIT_KEY_REQUIRES_ONE_LOCATOR};
pub use model::{Paper, Publication, ResolvedPaper};
pub use provider::{MetadataProvider, ProviderError};
