//! The provider contract, provider-neutral results, registry, and local provider.
//!
//! Providers differ in capability, not in kind. This crate defines the
//! object-safe contract every provider implements, the ordered registry that
//! dispatches to them, and the local provider that ingests user-supplied
//! BibTeX and never refreshes.
//!
//! Two rules are enforced here rather than merely documented elsewhere.
//!
//! **Absence advances; failure stops.** Not-found and unsupported-locator are
//! outcomes, so the next provider in roster order is offered them. A retrieval
//! or mapping error is not: a temporary outage must never silently decide which
//! provider permanently owns a record's BibTeX and citation key.
//!
//! **Nothing outside this crate knows that `local` is special.** Refresh does
//! not test for it and skip; it asks every provider to refresh its records, and
//! the local provider reports that it refreshes none. A `provider.name() ==
//! "local"` test appearing in a command means the abstraction has failed.
#![warn(missing_docs)]

mod contract;
mod error;
mod local;
mod outcome;
mod registry;
pub mod testing;

pub use contract::{Provider, ProviderCapabilities, ProviderFuture};
pub use error::{MappingError, ProviderError, RetrievalError};
pub use local::{LOCAL_PROVIDER, LocalProvider};
pub use outcome::{
    PayloadItem, PayloadRequest, ProviderMetadata, RefreshItem, RefreshRequest, RefreshState,
    Resolution,
};
pub use registry::{LocatorOutcome, ProviderRegistry, RegistryError};
