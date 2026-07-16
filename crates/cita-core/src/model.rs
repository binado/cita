use serde::{Deserialize, Deserializer, Serialize};

/// Provider name used by INSPIRE-backed records. `Locator::Inspire` selectors
/// match records whose `source` equals this and whose `source_id` is the
/// INSPIRE record id.
pub const INSPIRE_SOURCE: &str = "inspire";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Publication {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<String>,
    /// Year the journal version appeared, which may differ from the
    /// citation-display year on the record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
}

impl Publication {
    pub fn is_journal(&self) -> bool {
        self.journal.is_some()
    }
}

/// Stored form of a paper: everything about it except the citation key,
/// which lives as the map key wherever records are collected.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PaperRecord {
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collaborations: Vec<String>,
    /// Citation-display year (often the preprint year rather than the
    /// journal year).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub document_types: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Provider name, e.g. [`INSPIRE_SOURCE`].
    pub source: String,
    /// Provider-native identifier (for INSPIRE, the literature record id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arxiv_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dois: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preprint_date: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_publication",
        skip_serializing_if = "Option::is_none"
    )]
    pub publication: Option<Publication>,
}

fn deserialize_publication<'de, D>(deserializer: D) -> Result<Option<Publication>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Publication>::deserialize(deserializer)?
        .filter(|publication| publication != &Publication::default()))
}

/// What a metadata provider returns: a record plus an advisory key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedPaper {
    /// Provider-suggested citation key (e.g. an INSPIRE texkey). Consumed
    /// when choosing the key at insertion time; never stored on the record.
    pub suggested_key: Option<String>,
    pub record: PaperRecord,
}
