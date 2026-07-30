//! Closed provider selection, construction, local ingestion, and refresh.
//!
//! [`Provider`] is the one closed provider enum for the whole workspace,
//! defined in `bibi-core` and re-exported here. Resolution selects exactly one
//! provider and never falls back.
#![warn(missing_docs)]

mod facade;
mod local;

pub mod testing;

pub use bibi_core::provider::{MappingError, ProviderError, RetrievalError};
pub use facade::{
    Error, Provider, Providers, ProvidersBuilder, RefreshOptions, RefreshOutcome, RefreshTarget,
    RefreshedItem, ResolveItem,
};
