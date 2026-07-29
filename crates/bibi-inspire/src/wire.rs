//! INSPIRE's response shape, private to mapping.
//!
//! Unknown fields are tolerated in this direction — the opposite of the
//! manifest's policy — because bibi consumes a foreign, evolving schema and
//! wants a subset of it. A field INSPIRE adds must never break a refresh.

use serde::{Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
pub(crate) struct SearchResponse {
    #[serde(default)]
    pub(crate) hits: Hits,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Hits {
    #[serde(default)]
    pub(crate) hits: Vec<LiteratureRecord>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LiteratureRecord {
    /// INSPIRE serializes this as a number or a string depending on the route.
    #[serde(default, deserialize_with = "flexible_id")]
    pub(crate) id: Option<u64>,
    /// The change token bibi stores as a revision.
    #[serde(default)]
    pub(crate) updated: Option<String>,
    #[serde(default)]
    pub(crate) metadata: Metadata,
}

impl LiteratureRecord {
    /// The stable record id, however this response spelled it.
    pub(crate) fn record_id(&self) -> Option<u64> {
        self.id.or(self.metadata.control_number)
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Metadata {
    #[serde(default)]
    pub(crate) control_number: Option<u64>,
    #[serde(default)]
    pub(crate) texkeys: Vec<String>,
    #[serde(default)]
    pub(crate) titles: Vec<Title>,
    #[serde(default)]
    pub(crate) authors: Vec<Author>,
    #[serde(default)]
    pub(crate) collaborations: Vec<Collaboration>,
    #[serde(default)]
    pub(crate) publication_info: Vec<PublicationInfo>,
    #[serde(default)]
    pub(crate) preprint_date: Option<String>,
    #[serde(default)]
    pub(crate) arxiv_eprints: Vec<Eprint>,
    #[serde(default)]
    pub(crate) dois: Vec<Doi>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Title {
    #[serde(default)]
    pub(crate) title: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Author {
    #[serde(default)]
    pub(crate) full_name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Collaboration {
    #[serde(default)]
    pub(crate) value: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PublicationInfo {
    #[serde(default)]
    pub(crate) year: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Eprint {
    #[serde(default)]
    pub(crate) value: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Doi {
    #[serde(default)]
    pub(crate) value: String,
}

fn flexible_id<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok())))
}
