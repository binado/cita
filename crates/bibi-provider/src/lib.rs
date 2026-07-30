//! Closed provider selection, construction, local ingestion, and refresh.
//!
//! The manifest keeps open-ended [`bibi_core::ProviderName`] provenance, while operations
//! that call installed implementations go through the closed [`Provider`] enum.
//! Resolution selects exactly one provider and never falls back.
#![warn(missing_docs)]

mod facade;
mod local;

pub mod testing;

pub use bibi_core::provider::{MappingError, ProviderError, RetrievalError};
pub use facade::{
    Error, Provider, Providers, ProvidersBuilder, RefreshOptions, RefreshOutcome, RefreshTarget,
    RefreshedItem, ResolveItem,
};
pub use local::LOCAL_PROVIDER;
