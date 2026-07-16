//! Provider-neutral vocabulary for Cita: paper models, locators,
//! citation-key helpers, and the `MetadataProvider` trait.

mod key;
mod locator;
mod model;
mod provider;

pub use key::{fallback_key, validate_key};
pub use locator::{Locator, normalize_arxiv, normalize_doi, strip_arxiv_version};
pub use model::{INSPIRE_SOURCE, PaperRecord, Publication, ResolvedPaper};
pub use provider::{MetadataProvider, ProviderError};
