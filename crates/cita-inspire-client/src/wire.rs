use crate::{Error, InspireSnapshot};
use cita_bibliography::{BibtexSnapshot, project_bibtex};
use cita_core::{
    Identifiers, ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi,
};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};

/// The supported subset of an INSPIRE literature JSON record.
///
/// Unknown response fields are deliberately ignored. This type models only
/// the metadata needed for transient reference projection and for attaching an
/// authoritative BibTeX snapshot.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ApiLiteratureRecord {
    /// Stable record identifier, when the top-level `id` is numeric.
    #[serde(default, deserialize_with = "deserialize_optional_id")]
    pub id: Option<u64>,
    /// Provider update timestamp.
    #[serde(default)]
    pub updated: Option<String>,
    /// Supported literature metadata.
    pub metadata: ApiLiteratureMetadata,
}

impl ApiLiteratureRecord {
    /// Return the stable record identifier, falling back to `control_number`.
    pub fn record_id(&self) -> Option<u64> {
        self.id.or(self.metadata.control_number)
    }

    /// Attach an already validated standalone BibTeX entry.
    ///
    /// The entry key must be one of this record's INSPIRE texkeys. If the JSON
    /// supplies DOI or arXiv identities, at least one must also occur in the
    /// BibTeX projection. Curated identifiers are selected deterministically
    /// from the normalized JSON/BibTeX intersection.
    pub fn attach_bibtex(self, bibtex: BibtexSnapshot) -> Result<InspireSnapshot, Error> {
        let record_id = self
            .record_id()
            .ok_or_else(|| Error::Malformed("record has no numeric id".into()))?;
        let updated = self
            .updated
            .clone()
            .ok_or_else(|| Error::Malformed("record has no update timestamp".into()))?;
        let bibtex_key = bibtex
            .key()
            .map_err(|error| Error::Malformed(error.to_string()))?;
        if !self.metadata.texkeys.contains(&bibtex_key) {
            return Err(Error::Malformed(format!(
                "BibTeX key `{bibtex_key}` is not one of the INSPIRE texkeys"
            )));
        }
        let bib_reference =
            project_bibtex(&bibtex.bibtex).map_err(|error| Error::Malformed(error.to_string()))?;
        let arxiv = self.normalized_arxiv();
        let dois = self.normalized_dois();
        if !identity_matches(&arxiv, &dois, &bib_reference) {
            return Err(Error::Malformed(
                "INSPIRE JSON and BibTeX do not identify the same record".into(),
            ));
        }
        Ok(InspireSnapshot {
            record_id,
            updated,
            texkey: bibtex_key,
            arxiv: curated(arxiv, &bib_reference.identifiers.arxiv),
            doi: curated(dois, &bib_reference.identifiers.dois),
            bibtex: bibtex.bibtex,
        })
    }

    fn normalized_arxiv(&self) -> Vec<String> {
        unique(
            self.metadata
                .arxiv_eprints
                .iter()
                .map(|value| normalize_arxiv(&value.value)),
        )
    }

    fn normalized_dois(&self) -> Vec<String> {
        unique(
            self.metadata
                .dois
                .iter()
                .map(|value| normalize_doi(&value.value)),
        )
    }
}

impl ReferenceSource for ApiLiteratureRecord {
    fn project(&self) -> Result<Reference, ProjectionError> {
        let title = self
            .metadata
            .titles
            .iter()
            .map(|value| value.title.trim())
            .find(|value| !value.is_empty())
            .ok_or(ProjectionError::MissingTitle)?
            .to_owned();
        let record_id = self
            .record_id()
            .ok_or_else(|| ProjectionError::Invalid("INSPIRE record has no numeric id".into()))?;
        let mut providers = BTreeMap::new();
        providers.insert("inspire".to_owned(), vec![record_id.to_string()]);
        Ok(Reference {
            title,
            identifiers: Identifiers {
                dois: self.normalized_dois(),
                arxiv: self.normalized_arxiv(),
                providers,
            },
            ..Reference::default()
        })
    }
}

/// The supported metadata subset of an INSPIRE literature JSON record.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct ApiLiteratureMetadata {
    /// Stable control number used when the top-level record ID is unavailable.
    #[serde(default)]
    pub control_number: Option<u64>,
    /// Candidate display titles in provider order.
    #[serde(default)]
    pub titles: Vec<ApiTitle>,
    /// Provider citation keys accepted for BibTeX matching.
    #[serde(default)]
    pub texkeys: Vec<String>,
    /// arXiv identifiers supplied by INSPIRE.
    #[serde(default)]
    pub arxiv_eprints: Vec<ApiArxivEprint>,
    /// DOI identifiers supplied by INSPIRE.
    #[serde(default)]
    pub dois: Vec<ApiDoi>,
}

/// A title value in INSPIRE literature metadata.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ApiTitle {
    /// Title text.
    pub title: String,
}

/// An arXiv identifier value in INSPIRE literature metadata.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ApiArxivEprint {
    /// Provider-supplied arXiv identifier.
    pub value: String,
}

/// A DOI value in INSPIRE literature metadata.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ApiDoi {
    /// Provider-supplied DOI.
    pub value: String,
}

#[derive(Deserialize)]
pub(crate) struct SearchResponse {
    pub(crate) hits: SearchHits,
}

#[derive(Deserialize)]
pub(crate) struct SearchHits {
    #[serde(default)]
    pub(crate) hits: Vec<ApiLiteratureRecord>,
}

fn deserialize_optional_id<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok())))
}

fn identity_matches(arxiv: &[String], dois: &[String], bib: &Reference) -> bool {
    (arxiv.is_empty() && dois.is_empty())
        || dois.iter().any(|id| bib.identifiers.dois.contains(id))
        || arxiv.iter().any(|id| bib.identifiers.arxiv.contains(id))
}

fn unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn curated(json: Vec<String>, bib: &[String]) -> Option<String> {
    unique(json.into_iter().filter(|id| bib.contains(id)))
        .into_iter()
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(value: serde_json::Value) -> ApiLiteratureRecord {
        serde_json::from_value(value).unwrap()
    }

    fn snapshot(bibtex: &str) -> BibtexSnapshot {
        BibtexSnapshot::new(bibtex.into()).unwrap()
    }

    #[test]
    fn deserializes_unknown_fields_and_numeric_or_string_ids() {
        for id in [serde_json::json!(1124337), serde_json::json!("1124337")] {
            let record = record(serde_json::json!({
                "id": id,
                "updated": "2025-01-01",
                "links": {"self": "ignored"},
                "metadata": {
                    "titles": [{"title": "First", "subtitle": "ignored"}],
                    "authors": [{"full_name": "Aad, G."}],
                    "texkeys": ["Aad:2012tfa"],
                    "arxiv_eprints": [
                        {"value": "1207.7214v2", "categories": ["hep-ex"]},
                        {"value": "1207.7214"}
                    ],
                    "dois": [
                        {"value": "10.1/ABC", "material": "publication"},
                        {"value": "10.1/abc"}
                    ],
                    "new_api_field": [1, 2, 3]
                }
            }));
            assert_eq!(record.record_id(), Some(1124337));
            let projected = record.project().unwrap();
            assert_eq!(projected.title, "First");
            assert_eq!(projected.identifiers.arxiv, ["1207.7214"]);
            assert_eq!(projected.identifiers.dois, ["10.1/abc"]);
        }
    }

    #[test]
    fn falls_back_to_control_number_for_missing_or_non_numeric_id() {
        for id in [serde_json::Value::Null, serde_json::json!("not-a-number")] {
            let record = record(serde_json::json!({
                "id": id,
                "metadata": {
                    "control_number": 42,
                    "titles": [{"title": "Title"}]
                }
            }));
            assert_eq!(record.record_id(), Some(42));
            assert_eq!(
                record
                    .project()
                    .unwrap()
                    .identifiers
                    .providers
                    .get("inspire"),
                Some(&vec!["42".to_owned()])
            );
        }
    }

    #[test]
    fn projection_uses_first_non_empty_title_and_requires_one() {
        let with_title = record(serde_json::json!({
            "id": 42,
            "metadata": {"titles": [{"title": "  "}, {"title": " Second "}]}
        }));
        assert_eq!(with_title.project().unwrap().title, "Second");

        let without_title = record(serde_json::json!({
            "id": 42,
            "metadata": {"titles": [{"title": "  "}]}
        }));
        assert_eq!(without_title.project(), Err(ProjectionError::MissingTitle));
    }

    #[test]
    fn attach_bibtex_builds_snapshot_with_matching_alternate_texkey() {
        let record = record(serde_json::json!({
            "id": "42",
            "updated": "2026-01-01",
            "metadata": {
                "texkeys": ["First:2026", "Second:2026"],
                "arxiv_eprints": [{"value": "2401.00001v2"}],
                "dois": [{"value": "10.1/X"}]
            }
        }));
        let attached = record
            .attach_bibtex(snapshot(
                "@article{Second:2026,title={Real title},eprint={2401.00001},doi={10.1/x}}",
            ))
            .unwrap();
        assert_eq!(attached.record_id, 42);
        assert_eq!(attached.texkey, "Second:2026");
        assert_eq!(attached.arxiv.as_deref(), Some("2401.00001"));
        assert_eq!(attached.doi.as_deref(), Some("10.1/x"));
    }

    #[test]
    fn attach_bibtex_rejects_unmatched_texkey_and_identity_mismatch() {
        let unmatched = record(serde_json::json!({
            "id": 42,
            "updated": "2026-01-01",
            "metadata": {"texkeys": ["Expected"]}
        }))
        .attach_bibtex(snapshot("@misc{Other,title={T}}"))
        .unwrap_err();
        assert!(
            unmatched
                .to_string()
                .contains("is not one of the INSPIRE texkeys")
        );

        let mismatch = record(serde_json::json!({
            "id": 42,
            "updated": "2026-01-01",
            "metadata": {
                "texkeys": ["Key"],
                "dois": [{"value": "10.1/json"}]
            }
        }))
        .attach_bibtex(snapshot("@misc{Key,title={T},doi={10.1/bibtex}}"))
        .unwrap_err();
        assert!(
            mismatch
                .to_string()
                .contains("do not identify the same record")
        );
    }

    #[test]
    fn attach_bibtex_accepts_identifier_free_records() {
        let attached = record(serde_json::json!({
            "id": 42,
            "updated": "2026-01-01",
            "metadata": {"texkeys": ["Key"]}
        }))
        .attach_bibtex(snapshot("@misc{Key,title={T},doi={10.1/bibtex}}"))
        .unwrap();
        assert_eq!(attached.arxiv, None);
        assert_eq!(attached.doi, None);
    }

    #[test]
    fn attach_bibtex_selects_deterministic_identifier_intersection() {
        let attached = record(serde_json::json!({
            "id": 42,
            "updated": "2026-01-01",
            "metadata": {
                "texkeys": ["Key"],
                "arxiv_eprints": [
                    {"value": "2401.00002"},
                    {"value": "2401.00001"}
                ],
                "dois": [
                    {"value": "10.1/ZZZ"},
                    {"value": "10.1/AAA"}
                ]
            }
        }))
        .attach_bibtex(snapshot(
            "@misc{Key,title={T},eprint={2401.00002},doi={10.1/zzz}}",
        ))
        .unwrap();
        assert_eq!(attached.arxiv.as_deref(), Some("2401.00002"));
        assert_eq!(attached.doi.as_deref(), Some("10.1/zzz"));
    }
}
