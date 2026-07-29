//! The global, derived, disposable document cache.
//!
//! Artifacts are addressed by normalized arXiv identifier and kind, never by
//! record identity, so two projects citing one paper share a copy and a cached
//! file is meaningful on its own. Nothing in a manifest refers to a cache path,
//! so rename, removal, and provider migration require no bookkeeping here.
//!
//! The cache is derived and disposable. Removing a record does not evict its
//! documents — another project may want them, and there is no registry that
//! could say otherwise — so eviction is an explicit global operation.
#![warn(missing_docs)]

mod clean;
mod error;
mod path;
mod source;
mod store;

pub use clean::{CleanMode, CleanReport};
pub use error::Error;
pub use path::{ArtifactKind, DOCUMENTS};
pub use source::ArchiveLimits;
pub use store::{DocumentStore, DocumentStoreBuilder, FetchOutcome, FetchPolicy, pdf_url};
