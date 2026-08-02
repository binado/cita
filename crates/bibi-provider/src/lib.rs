//! Closed provider selection and strict batched resolution.
#![warn(missing_docs)]

mod facade;

pub mod testing;

pub use bibi_core::{
    ProviderName,
    remote::{MappingError, ProviderError, RetrievalError},
};
pub use facade::{Error, Providers, ProvidersBuilder};
