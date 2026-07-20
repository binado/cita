use crate::Locator;
use std::{collections::BTreeMap, future::Future};
use thiserror::Error;

/// The provider-neutral semantic view used by commands and identity checks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Reference {
    pub title: String,
    pub authors: Vec<String>,
    pub collaborations: Vec<String>,
    pub year: Option<i32>,
    pub identifiers: Identifiers,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Identifiers {
    /// Normalized DOI values, in whatever order the producing source stored.
    pub dois: Vec<String>,
    /// Normalized, versionless arXiv identifiers.
    pub arxiv: Vec<String>,
    /// Namespaced provider identities, e.g. `inspire = ["1124337"]`.
    pub providers: BTreeMap<String, Vec<String>>,
}

pub trait ReferenceSource {
    fn project(&self) -> Result<Reference, ProjectionError>;
}

/// Provider contract. Snapshots remain provider-owned; callers consume their
/// neutral projection and store the complete snapshot separately.
pub trait MetadataProvider: Send + Sync {
    type Snapshot: ReferenceSource + Clone + Send + Sync;

    fn resolve(
        &self,
        locator: &Locator,
    ) -> impl Future<Output = Result<Self::Snapshot, ProviderError>> + Send;

    fn refresh(
        &self,
        provider_ids: &[String],
    ) -> impl Future<Output = Result<Vec<Self::Snapshot>, ProviderError>> + Send;
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProjectionError {
    #[error("source has no title")]
    MissingTitle,
    #[error("invalid source metadata: {0}")]
    Invalid(String),
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("invalid locator: {0}")]
    InvalidLocator(String),
    #[error("no record found for {0}")]
    NotFound(String),
    #[error("metadata provider request failed: {0}")]
    Request(String),
    #[error("metadata provider returned malformed data: {0}")]
    Malformed(String),
}
