use crate::{Error, snapshot::SelectedRecord};
use serde::Deserialize;

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
            title: metadata.titles.into_iter().next().map(|value| value.title),
            arxiv: metadata
                .arxiv_eprints
                .into_iter()
                .map(|value| value.value)
                .collect(),
            doi: metadata.dois.into_iter().map(|value| value.value).collect(),
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
    texkeys: Vec<String>,
    #[serde(default)]
    arxiv_eprints: Vec<ApiArxivEprint>,
    #[serde(default)]
    dois: Vec<ApiDoi>,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiTitle {
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiArxivEprint {
    value: String,
}

#[derive(Clone, Debug, Deserialize)]
struct ApiDoi {
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

#[cfg(test)]
mod tests {
    use super::*;
    use cita_core::ReferenceSource;

    #[test]
    fn wire_accepts_unknown_fields_and_selects_the_strict_subset() {
        let record: LiteratureRecord = serde_json::from_value(serde_json::json!({
            "id": "1124337",
            "updated": "2025-01-01",
            "links": {"self": "ignored"},
            "metadata": {
                "titles": [{"title": "First", "subtitle": "ignored"}],
                "authors": [{"full_name": "Aad, G.", "raw_affiliations": [{"value": "x"}]}],
                "texkeys": ["Aad:2012tfa"],
                "arxiv_eprints": [{"value": "1207.7214v2", "categories": ["hep-ex"]}],
                "dois": [{"value": "10.1/ABC", "material": "publication"}],
                "publication_info": [{"journal_title": "JHEP", "year": 2012}],
                "new_api_field": [1, 2, 3]
            }
        }))
        .unwrap();
        let selected = record.into_selected().unwrap();
        assert_eq!(selected.record_id(), 1124337);
        assert_eq!(selected.texkeys(), ["Aad:2012tfa"]);
        let projected = selected.project().unwrap();
        assert_eq!(projected.title, "First");
        assert_eq!(projected.identifiers.arxiv, ["1207.7214"]);
        assert_eq!(projected.identifiers.dois, ["10.1/abc"]);
    }
}
