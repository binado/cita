use crate::{
    Error,
    snapshot::{
        ArxivEprint, Author, Collaboration, Doi, PublicationInfo, SelectedRecord, Title, UrlValue,
    },
};
use serde::{Deserialize, Deserializer};

/// Permissive INSPIRE API response types. Unknown wire fields are deliberately
/// ignored; conversion selects the strict subset stored in `SelectedRecord`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LiteratureRecord {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    updated: Option<String>,
    metadata: LiteratureMetadata,
}

impl LiteratureRecord {
    pub(crate) fn record_id(&self) -> Option<u64> {
        self.id
            .as_u64()
            .or_else(|| self.id.as_str()?.parse().ok())
            .or(self.metadata.control_number)
    }

    pub(crate) fn into_selected(self) -> Result<SelectedRecord, Error> {
        let record_id = self
            .record_id()
            .ok_or_else(|| Error::Malformed("record has no numeric id".into()))?;
        let metadata = self.metadata;
        Ok(SelectedRecord {
            record_id,
            updated: self
                .updated
                .ok_or_else(|| Error::Malformed("record has no update timestamp".into()))?,
            texkeys: metadata.texkeys,
            titles: metadata
                .titles
                .into_iter()
                .map(|value| Title { title: value.title })
                .collect(),
            authors: metadata
                .authors
                .into_iter()
                .map(|value| Author {
                    full_name: value.full_name,
                    role: value.role,
                })
                .collect(),
            collaborations: metadata
                .collaborations
                .into_iter()
                .map(|value| Collaboration { value: value.value })
                .collect(),
            publication_info: metadata
                .publication_info
                .into_iter()
                .map(|value| PublicationInfo {
                    journal_title: value.journal_title,
                    journal_volume: value.journal_volume,
                    journal_issue: value.journal_issue,
                    page_start: value.page_start,
                    page_end: value.page_end,
                    artid: value.artid,
                    year: value.year,
                    hidden: value.hidden,
                    curated_relation: value.curated_relation,
                })
                .collect(),
            arxiv_eprints: metadata
                .arxiv_eprints
                .into_iter()
                .map(|value| ArxivEprint {
                    value: value.value,
                    categories: value.categories,
                })
                .collect(),
            dois: metadata
                .dois
                .into_iter()
                .map(|value| Doi { value: value.value })
                .collect(),
            urls: metadata
                .urls
                .into_iter()
                .map(|value| UrlValue { value: value.value })
                .collect(),
            document_types: metadata.document_types,
            preprint_date: metadata.preprint_date,
            earliest_date: metadata.earliest_date,
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
struct LiteratureMetadata {
    #[serde(default)]
    control_number: Option<u64>,
    #[serde(default)]
    titles: Vec<ApiTitle>,
    #[serde(default)]
    authors: Vec<ApiAuthor>,
    #[serde(default)]
    collaborations: Vec<ApiCollaboration>,
    #[serde(default)]
    texkeys: Vec<String>,
    #[serde(default)]
    publication_info: Vec<ApiPublicationInfo>,
    #[serde(default)]
    arxiv_eprints: Vec<ApiArxivEprint>,
    #[serde(default)]
    dois: Vec<ApiDoi>,
    #[serde(default)]
    urls: Vec<ApiUrlValue>,
    #[serde(default, alias = "document_type")]
    document_types: Vec<String>,
    #[serde(default)]
    preprint_date: Option<String>,
    #[serde(default)]
    earliest_date: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiTitle {
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiAuthor {
    full_name: String,
    #[serde(default, deserialize_with = "one_or_many")]
    role: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiCollaboration {
    value: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiPublicationInfo {
    #[serde(default)]
    journal_title: Option<String>,
    #[serde(default)]
    journal_volume: Option<String>,
    #[serde(default)]
    journal_issue: Option<String>,
    #[serde(default)]
    page_start: Option<String>,
    #[serde(default)]
    page_end: Option<String>,
    #[serde(default)]
    artid: Option<String>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    curated_relation: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiArxivEprint {
    value: String,
    #[serde(default)]
    categories: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiDoi {
    value: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiUrlValue {
    value: String,
}

#[derive(Deserialize)]
pub(crate) struct SearchResponse {
    pub(crate) hits: SearchHits,
}

#[derive(Deserialize)]
pub(crate) struct SearchHits {
    #[serde(default)]
    pub(crate) hits: Vec<LiteratureRecord>,
}

fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use cita_core::ReferenceSource;

    #[test]
    fn wire_accepts_unknown_fields_at_every_level() {
        let record: LiteratureRecord = serde_json::from_value(serde_json::json!({
            "id": "1124337",
            "updated": "2025-01-01",
            "links": {"self": "ignored"},
            "metadata": {
                "titles": [{"title": "First", "subtitle": "ignored"}],
                "authors": [
                    {"full_name": "Aad, G.", "raw_affiliations": [{"value":"ignored"}]},
                    {"full_name": "Writer", "role": ["author"]},
                    {"full_name": "Editor", "role": "editor"}
                ],
                "texkeys": ["Aad:2012tfa"],
                "arxiv_eprints": [{
                    "value": "1207.7214v2", "categories": ["hep-ex"], "extra": true
                }],
                "dois": [{"value": "10.1/ABC", "material": "publication"}],
                "publication_info": [{
                    "journal_title": "JHEP", "year": 2012, "artid": "1",
                    "curated_relation": true, "unknown_nested": {"x": 1}
                }],
                "new_api_field": [1, 2, 3]
            }
        }))
        .unwrap();
        let selected = record.into_selected().unwrap();
        let projected = selected.project().unwrap();
        assert_eq!(projected.year, Some(2012));
        assert_eq!(projected.authors, ["Aad, G.", "Writer"]);
        let snapshot = selected.into_snapshot(
            "@article{Aad:2012tfa,title={First},doi={10.1/ABC},eprint={1207.7214}}".into(),
        );
        assert_eq!(snapshot.project().unwrap(), projected);
        snapshot.validate().unwrap();
    }
}
