use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type ExtraFields = BTreeMap<String, Value>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LiteratureRecord {
    #[serde(default)]
    pub id: Option<String>,
    pub metadata: LiteratureMetadata,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LiteratureMetadata {
    #[serde(default)]
    pub titles: Vec<Title>,
    #[serde(default)]
    pub authors: Vec<Author>,
    #[serde(default)]
    pub collaborations: Vec<Collaboration>,
    #[serde(default)]
    pub texkeys: Vec<String>,
    #[serde(default)]
    pub publication_info: Vec<PublicationInfo>,
    #[serde(default)]
    pub arxiv_eprints: Vec<ArxivEprint>,
    #[serde(default)]
    pub dois: Vec<Doi>,
    #[serde(default, alias = "document_type")]
    pub document_types: Vec<String>,
    #[serde(default)]
    pub urls: Vec<UrlValue>,
    #[serde(default)]
    pub preprint_date: Option<String>,
    #[serde(default)]
    pub earliest_date: Option<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Title {
    pub title: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Author {
    pub full_name: String,
    #[serde(default, deserialize_with = "one_or_many")]
    pub role: Vec<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Collaboration {
    pub value: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PublicationInfo {
    #[serde(default)]
    pub journal_title: Option<String>,
    #[serde(default)]
    pub journal_volume: Option<String>,
    #[serde(default)]
    pub journal_issue: Option<String>,
    #[serde(default)]
    pub page_start: Option<String>,
    #[serde(default)]
    pub page_end: Option<String>,
    #[serde(default)]
    pub artid: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub curated_relation: bool,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArxivEprint {
    pub value: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Doi {
    pub value: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UrlValue {
    pub value: String,
    #[serde(flatten)]
    pub extra: ExtraFields,
}

fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        One(String),
        Many(Vec<String>),
    }
    Option::<Value>::deserialize(deserializer).map(|value| match value {
        None => Vec::new(),
        Some(Value::One(value)) => vec![value],
        Some(Value::Many(values)) => values,
    })
}
