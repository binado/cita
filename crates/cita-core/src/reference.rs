use crate::Locator;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, future::Future};
use thiserror::Error;

/// The provider-neutral semantic view used by commands and identity checks.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collaborations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<Publication>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_category: Option<String>,
    #[serde(default)]
    pub identifiers: Identifiers,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Identifiers {
    /// Normalized DOI values. BTree-backed vectors keep serialized order stable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dois: Vec<String>,
    /// Normalized, versionless arXiv identifiers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arxiv: Vec<String>,
    /// Namespaced provider identities, e.g. `inspire = ["1124337"]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Publication {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
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
