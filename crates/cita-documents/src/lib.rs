//! arXiv PDF and source download and local cache management for cita.
#![warn(missing_docs)]

mod error;
mod source;
mod store;

pub use error::Error;
pub use store::{
    ArtifactKind, DocumentStore, DocumentStoreBuilder, FetchOutcome, FetchPolicy, arxiv_pdf_url,
};
