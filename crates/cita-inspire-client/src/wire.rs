use crate::{Error, InspireSnapshot};
use cita_bibliography::{BibtexSnapshot, project_bibtex};
use cita_core::{
    Identifiers, ProjectionError, Reference, ReferenceSource, normalize_arxiv, normalize_doi,
};
use serde::{Deserialize, Deserializer};
use std::collections::{BTreeMap, BTreeSet};

/// Individually curated authors are capped at this many: a collaboration
/// record can list thousands, and a manifest needs a display list rather than
/// the complete author roster.
const MAX_AUTHORS: usize = 10;

/// Metadata fields requested via `?fields=` on the batched search endpoint
/// (see `Client::search_urls`). Every `#[serde(default)]` field read from
/// [`ApiLiteratureMetadata`] must have its INSPIRE name listed here, or it
/// silently deserializes empty instead of erroring.
pub(crate) const PROJECTION_FIELDS: &[&str] = &[
    "control_number",
    "titles",
    "texkeys",
    "arxiv_eprints",
    "dois",
    "document_type",
    "authors",
    "collaborations",
    "publication_info",
    "preprint_date",
    "imprints",
];

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
    /// BibTeX projection: this cross-check proves the blob belongs to this
    /// record, but the returned snapshot's reference is projected from the
    /// JSON alone, which is authoritative outright, not merely the shared
    /// subset of the two sources.
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
        // This projects the BibTeX purely to cross-check identity against the
        // JSON record; the returned snapshot's reference comes from
        // `self.project()`, which is authoritative.
        let bib_reference =
            project_bibtex(&bibtex.bibtex).map_err(|error| Error::Malformed(error.to_string()))?;
        let arxiv = self.normalized_arxiv();
        let dois = self.normalized_dois();
        if !identity_matches(&arxiv, &dois, &bib_reference) {
            return Err(Error::Malformed(
                "INSPIRE JSON and BibTeX do not identify the same record".into(),
            ));
        }
        let reference = self
            .project()
            .map_err(|error| Error::Malformed(error.to_string()))?;
        Ok(InspireSnapshot {
            record_id,
            updated,
            texkey: bibtex_key,
            bibtex: bibtex.bibtex,
            reference,
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

    /// Join every `document_type` value into one display string.
    ///
    /// Sorted and deduped before joining: INSPIRE does not guarantee a stable
    /// order for a multi-valued `document_type` (observed both
    /// `["note","article"]` and `["article","note"]` for the same kind of
    /// record), and an unsorted join would flip the stored value on refresh.
    fn entry_type(&self) -> Result<String, ProjectionError> {
        if self.metadata.document_type.is_empty() {
            return Err(ProjectionError::Invalid(
                "record has no document type".into(),
            ));
        }
        let mut values = self.metadata.document_type.clone();
        values.sort();
        values.dedup();
        Ok(values.join(", "))
    }

    /// The curated publication year, falling through to the preprint
    /// submission date and then the imprint date.
    fn year(&self) -> Option<i32> {
        self.metadata
            .publication_info
            .iter()
            .find_map(|info| info.year)
            .or_else(|| leading_year(self.metadata.preprint_date.as_deref()))
            .or_else(|| {
                self.metadata
                    .imprints
                    .iter()
                    .find_map(|imprint| leading_year(imprint.date.as_deref()))
            })
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
        let entry_type = self.entry_type()?;
        let record_id = self
            .record_id()
            .ok_or_else(|| ProjectionError::Invalid("INSPIRE record has no numeric id".into()))?;
        let mut providers = BTreeMap::new();
        providers.insert("inspire".to_owned(), vec![record_id.to_string()]);
        Ok(Reference {
            entry_type,
            title,
            authors: self
                .metadata
                .authors
                .iter()
                .map(|author| author.full_name.clone())
                .take(MAX_AUTHORS)
                .collect(),
            collaborations: self
                .metadata
                .collaborations
                .iter()
                .map(|collaboration| collaboration.value.clone())
                .collect(),
            year: self.year(),
            identifiers: Identifiers {
                dois: self.normalized_dois(),
                arxiv: self.normalized_arxiv(),
                providers,
            },
        })
    }
}

fn leading_year(date: Option<&str>) -> Option<i32> {
    date?.get(0..4)?.parse().ok()
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
    /// Document type values, e.g. `["article"]` or `["thesis"]`.
    #[serde(default)]
    pub document_type: Vec<String>,
    /// Individually curated authors, in source order.
    #[serde(default)]
    pub authors: Vec<ApiAuthor>,
    /// Collaboration names.
    #[serde(default)]
    pub collaborations: Vec<ApiCollaboration>,
    /// Publication venue entries; the first with a curated year wins.
    #[serde(default)]
    pub publication_info: Vec<ApiPublicationInfo>,
    /// Preprint submission date, e.g. `"2012-07"`.
    #[serde(default)]
    pub preprint_date: Option<String>,
    /// Imprint entries, used as a last-resort year source.
    #[serde(default)]
    pub imprints: Vec<ApiImprint>,
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

/// An author value in INSPIRE literature metadata.
///
/// The live payload also carries `affiliations`, `ids`, `uuid`, `record`, and
/// `signature_block`; all are ignored, same as every other unknown field.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ApiAuthor {
    /// `Last, First` display name.
    pub full_name: String,
}

/// A collaboration value in INSPIRE literature metadata.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ApiCollaboration {
    /// Collaboration name.
    pub value: String,
}

/// A publication venue entry in INSPIRE literature metadata.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct ApiPublicationInfo {
    /// Curated publication year, when known.
    #[serde(default)]
    pub year: Option<i32>,
}

/// An imprint entry in INSPIRE literature metadata.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct ApiImprint {
    /// Imprint date, e.g. `"2012-07-15"`.
    #[serde(default)]
    pub date: Option<String>,
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
                    "document_type": ["article"],
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
            assert_eq!(projected.entry_type, "article");
            assert_eq!(projected.authors, ["Aad, G."]);
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
                    "titles": [{"title": "Title"}],
                    "document_type": ["article"]
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
            "metadata": {
                "titles": [{"title": "  "}, {"title": " Second "}],
                "document_type": ["article"]
            }
        }));
        assert_eq!(with_title.project().unwrap().title, "Second");

        let without_title = record(serde_json::json!({
            "id": 42,
            "metadata": {"titles": [{"title": "  "}], "document_type": ["article"]}
        }));
        assert_eq!(without_title.project(), Err(ProjectionError::MissingTitle));
    }

    #[test]
    fn document_type_is_sorted_deduped_and_joined_regardless_of_source_order() {
        let forward = record(serde_json::json!({
            "id": 42,
            "metadata": {
                "titles": [{"title": "T"}],
                "document_type": ["note", "article", "note"]
            }
        }));
        let reversed = record(serde_json::json!({
            "id": 42,
            "metadata": {
                "titles": [{"title": "T"}],
                "document_type": ["article", "note"]
            }
        }));
        // INSPIRE does not guarantee a stable order for this field; sampling
        // found the same pair in both orders for the same kind of record, so
        // an unsorted join would flip the stored value on refresh.
        assert_eq!(forward.project().unwrap().entry_type, "article, note");
        assert_eq!(reversed.project().unwrap().entry_type, "article, note");
    }

    #[test]
    fn empty_document_type_is_rejected() {
        let record = record(serde_json::json!({
            "id": 42,
            "metadata": {"titles": [{"title": "T"}]}
        }));
        assert_eq!(
            record.project(),
            Err(ProjectionError::Invalid(
                "record has no document type".into()
            ))
        );
    }

    #[test]
    fn authors_are_capped_at_the_maximum() {
        let authors = (0..(MAX_AUTHORS + 5))
            .map(|index| serde_json::json!({"full_name": format!("Author {index}")}))
            .collect::<Vec<_>>();
        let record = record(serde_json::json!({
            "id": 42,
            "metadata": {
                "titles": [{"title": "T"}],
                "document_type": ["article"],
                "authors": authors
            }
        }));
        let projected = record.project().unwrap();
        assert_eq!(projected.authors.len(), MAX_AUTHORS);
        assert_eq!(projected.authors[0], "Author 0");
        assert_eq!(projected.authors[MAX_AUTHORS - 1], "Author 9");
    }

    #[test]
    fn year_falls_through_publication_info_then_preprint_date_then_imprints() {
        fn year(metadata: serde_json::Value) -> Option<i32> {
            record(serde_json::json!({"id": 42, "metadata": metadata}))
                .project()
                .unwrap()
                .year
        }

        assert_eq!(
            year(serde_json::json!({
                "titles": [{"title": "T"}],
                "document_type": ["article"],
                "publication_info": [{"year": 2015}],
                "preprint_date": "2012-07",
                "imprints": [{"date": "2010-01-01"}]
            })),
            Some(2015)
        );
        assert_eq!(
            year(serde_json::json!({
                "titles": [{"title": "T"}],
                "document_type": ["article"],
                "preprint_date": "2012-07",
                "imprints": [{"date": "2010-01-01"}]
            })),
            Some(2012)
        );
        assert_eq!(
            year(serde_json::json!({
                "titles": [{"title": "T"}],
                "document_type": ["article"],
                "imprints": [{"date": "2010-01-01"}]
            })),
            Some(2010)
        );
        assert_eq!(
            year(serde_json::json!({
                "titles": [{"title": "T"}],
                "document_type": ["article"]
            })),
            None
        );
    }

    #[test]
    fn attach_bibtex_builds_snapshot_with_matching_alternate_texkey() {
        let record = record(serde_json::json!({
            "id": "42",
            "updated": "2026-01-01",
            "metadata": {
                "titles": [{"title": "Real title"}],
                "document_type": ["article"],
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
        assert_eq!(attached.reference.identifiers.arxiv, ["2401.00001"]);
        assert_eq!(attached.reference.identifiers.dois, ["10.1/x"]);
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
            "metadata": {
                "titles": [{"title": "T"}],
                "document_type": ["misc"],
                "texkeys": ["Key"]
            }
        }))
        .attach_bibtex(snapshot("@misc{Key,title={T},doi={10.1/bibtex}}"))
        .unwrap();
        assert!(attached.reference.identifiers.arxiv.is_empty());
        assert!(attached.reference.identifiers.dois.is_empty());
    }

    #[test]
    fn attached_reference_uses_the_complete_json_identifiers_not_just_the_bibtex_matched_one() {
        let attached = record(serde_json::json!({
            "id": 42,
            "updated": "2026-01-01",
            "metadata": {
                "titles": [{"title": "T"}],
                "document_type": ["article"],
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
            "@article{Key,title={T},eprint={2401.00002},doi={10.1/zzz}}",
        ))
        .unwrap();
        // The BibTeX only overlaps one arXiv ID and one DOI, enough to pass
        // the identity cross-check, but JSON is authoritative outright: every
        // curated identifier rides along, not just the intersection.
        assert_eq!(
            attached.reference.identifiers.arxiv,
            ["2401.00001", "2401.00002"]
        );
        assert_eq!(
            attached.reference.identifiers.dois,
            ["10.1/aaa", "10.1/zzz"]
        );
    }
}
