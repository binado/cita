use crate::{Locator, ResolvedPaper};
use async_trait::async_trait;
use thiserror::Error;

#[async_trait]
pub trait MetadataProvider: Send + Sync {
    async fn resolve(&self, locator: &Locator) -> Result<ResolvedPaper, ProviderError>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("invalid locator: {0}")]
    InvalidLocator(String),
    #[error("no INSPIRE record found for {0}; it may be new or outside INSPIRE coverage")]
    NotFound(String),
    #[error("metadata provider request failed: {0}")]
    Request(String),
    #[error("metadata provider returned malformed data: {0}")]
    Malformed(String),
}
