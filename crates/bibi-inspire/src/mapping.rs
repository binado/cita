//! Mapping: a pure function from a raw response to provider-neutral values.
//!
//! Every per-provider judgment call lives here — which title, which year,
//! which identifier is canonical — and none of it touches the network, a clock,
//! or any ambient state, so it is testable against captured responses.
//!
//! Mapping never reads the provider's BibTeX. The structured record is the
//! source; the BibTeX is a rendering of it, and deriving fields from a
//! rendering when the source is at hand is what this design refuses.

use crate::{
    error,
    join::DeclaredKeys,
    transport::{RawBibtex, RawJson},
    wire::{LiteratureRecord, SearchResponse},
};
use bibi_core::remote::{BibtexEntry, MappingError, parse_file};
use bibi_core::{ArxivId, Description, Doi, Identifiers, ProviderId, Revision};

/// One INSPIRE record, mapped.
///
/// Public because mapping is callable on its own, but it never crosses the
/// generic provider contract: a caller had to name this crate to obtain one, so
/// the coupling is visible and contained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappedRecord {
    /// The record id, rendered as decimal.
    pub provider_id: ProviderId,
    /// INSPIRE's `updated` timestamp, used as the change token.
    pub revision: Option<Revision>,
    /// Canonical normalized identifiers.
    pub identifiers: Identifiers,
    /// Advisory display data.
    pub description: Description,
    /// The texkeys INSPIRE declares for this record, for the payload join.
    pub texkeys: Vec<String>,
}

/// Parse a search response's envelope, leaving the hits unmapped.
///
/// Separate from [`map_search`] so a caller can map hits individually: one
/// malformed record then fails on its own instead of failing every record the
/// response carried. Only a response that cannot be read at all is an error
/// here.
pub(crate) fn parse_search(raw: &RawJson) -> Result<SearchResponse, MappingError> {
    serde_json::from_str(raw.as_str()).map_err(|error| MappingError::InvalidValue {
        provider: crate::error::provider(),
        kind: "JSON response",
        value: error.to_string(),
    })
}

/// Map a search response into records, in response order.
pub fn map_search(raw: &RawJson) -> Result<Vec<MappedRecord>, MappingError> {
    parse_search(raw)?
        .hits
        .hits
        .iter()
        .map(map_record)
        .collect()
}

/// Map one literature record.
pub(crate) fn map_record(record: &LiteratureRecord) -> Result<MappedRecord, MappingError> {
    let id = record
        .record_id()
        .ok_or_else(|| error::missing_field("id"))?;
    let metadata = &record.metadata;
    let title = metadata
        .titles
        .iter()
        .map(|title| title.title.trim())
        .find(|title| !title.is_empty())
        .ok_or_else(|| error::missing_field("titles"))?
        .to_owned();
    Ok(MappedRecord {
        provider_id: ProviderId::new(id.to_string())
            .map_err(|_| error::invalid_value("record id", id.to_string()))?,
        // A provider that cannot supply a token leaves it absent, and its
        // records are then treated as changed on every sync rather than
        // wrongly assumed current.
        revision: record
            .updated
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Revision::new(value).map_err(|_| error::invalid_value("revision", value)))
            .transpose()?,
        identifiers: Identifiers {
            // The first valid value wins, and an unparseable one is skipped
            // rather than failing the record: an identifier bibi cannot
            // normalize is one it could not have used anyway.
            doi: metadata
                .dois
                .iter()
                .find_map(|doi| Doi::new(&doi.value).ok()),
            arxiv: metadata
                .arxiv_eprints
                .iter()
                .find_map(|eprint| ArxivId::new(&eprint.value).ok()),
        },
        description: Description {
            title,
            authors: metadata
                .authors
                .iter()
                .map(|author| author.full_name.trim().to_owned())
                .filter(|author| !author.is_empty())
                .collect(),
            collaborations: metadata
                .collaborations
                .iter()
                .map(|collaboration| collaboration.value.trim().to_owned())
                .filter(|collaboration| !collaboration.is_empty())
                .collect(),
            // Publication year first, then the preprint date's year. Choosing
            // is the provider's policy precisely because the BibTeX would
            // otherwise decide it implicitly, differently, per renderer.
            year: metadata
                .publication_info
                .iter()
                .find_map(|info| info.year)
                .or_else(|| {
                    metadata
                        .preprint_date
                        .as_deref()
                        .and_then(|date| date.get(..4))
                        .and_then(|year| year.parse().ok())
                }),
        },
        texkeys: metadata
            .texkeys
            .iter()
            .map(|texkey| texkey.trim().to_owned())
            .filter(|texkey| !texkey.is_empty())
            .collect(),
    })
}

/// Read only the control numbers and texkeys a payload join needs.
///
/// Separate from [`map_search`] because the join's field set deliberately
/// excludes titles, and a record model that requires one would reject its own
/// narrowed response.
pub fn map_declared_keys(raw: &RawJson) -> Result<Vec<DeclaredKeys>, MappingError> {
    let response: SearchResponse =
        serde_json::from_str(raw.as_str()).map_err(|error| MappingError::InvalidValue {
            provider: crate::error::provider(),
            kind: "JSON response",
            value: error.to_string(),
        })?;
    response
        .hits
        .hits
        .iter()
        .map(|record| {
            let id = record
                .record_id()
                .ok_or_else(|| error::missing_field("id"))?;
            Ok(DeclaredKeys {
                provider_id: ProviderId::new(id.to_string())
                    .map_err(|_| error::invalid_value("record id", id.to_string()))?,
                texkeys: record
                    .metadata
                    .texkeys
                    .iter()
                    .map(|texkey| texkey.trim().to_owned())
                    .filter(|texkey| !texkey.is_empty())
                    .collect(),
            })
        })
        .collect()
}

/// Split a BibTeX response into validated standalone entries.
///
/// This is structural validation, not verification of INSPIRE's claims: the
/// entry will be re-keyed and rendered deterministically, so it has to be one
/// well-formed entry with a readable key. Rejecting a malformed response is
/// input validation; rejecting a well-formed one because a parse of it
/// disagreed with the structured record is the cross-check this design drops.
pub fn split_entries(raw: &RawBibtex) -> Result<Vec<BibtexEntry>, MappingError> {
    let entries = parse_file(raw.as_str()).map_err(error::invalid_payload)?;
    Ok(entries.into_iter().map(|entry| entry.payload).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(value: serde_json::Value) -> RawJson {
        RawJson::new(value.to_string())
    }

    fn one(value: serde_json::Value) -> MappedRecord {
        map_search(&json(serde_json::json!({"hits": {"hits": [value]}})))
            .unwrap()
            .remove(0)
    }

    #[test]
    fn maps_the_fields_the_record_model_uses() {
        let record = one(serde_json::json!({
            "id": 1124337,
            "updated": "2026-07-27T12:34:56+00:00",
            "links": {"self": "ignored"},
            "metadata": {
                "texkeys": ["Aad:2012tfa", "ATLAS:2012yve"],
                "titles": [{"title": "Observation of a new particle"}],
                "authors": [{"full_name": "Aad, G."}, {"full_name": " Abajyan, T. "}],
                "collaborations": [{"value": "ATLAS"}],
                "publication_info": [{"year": 2012}],
                "preprint_date": "2012-07-31",
                "arxiv_eprints": [{"value": "1207.7214", "categories": ["hep-ex"]}],
                "dois": [{"value": "10.1016/J.PhysLetB.2012.08.020"}],
                "a_field_added_next_year": [1, 2, 3]
            }
        }));
        assert_eq!(record.provider_id.as_str(), "1124337");
        assert_eq!(
            record.revision.as_ref().unwrap().as_str(),
            "2026-07-27T12:34:56+00:00"
        );
        assert_eq!(record.description.title, "Observation of a new particle");
        assert_eq!(record.description.authors, ["Aad, G.", "Abajyan, T."]);
        assert_eq!(record.description.collaborations, ["ATLAS"]);
        assert_eq!(record.description.year, Some(2012));
        assert_eq!(record.identifiers.arxiv.unwrap().as_str(), "1207.7214");
        assert_eq!(
            record.identifiers.doi.unwrap().as_str(),
            "10.1016/j.physletb.2012.08.020"
        );
        assert_eq!(record.texkeys, ["Aad:2012tfa", "ATLAS:2012yve"]);
    }

    #[test]
    fn falls_back_from_the_publication_year_to_the_preprint_date() {
        let record = one(serde_json::json!({
            "id": 1,
            "metadata": {
                "titles": [{"title": "T"}],
                "preprint_date": "1998-03-01"
            }
        }));
        assert_eq!(record.description.year, Some(1998));
    }

    #[test]
    fn falls_back_from_the_top_level_id_to_the_control_number() {
        for id in [serde_json::Value::Null, serde_json::json!("not a number")] {
            let record = one(serde_json::json!({
                "id": id,
                "metadata": {"control_number": 42, "titles": [{"title": "T"}]}
            }));
            assert_eq!(record.provider_id.as_str(), "42");
        }
        // A string id is still an id.
        let record = one(serde_json::json!({
            "id": "1124337",
            "metadata": {"titles": [{"title": "T"}]}
        }));
        assert_eq!(record.provider_id.as_str(), "1124337");
    }

    #[test]
    fn takes_the_first_usable_title_and_requires_one() {
        let record = one(serde_json::json!({
            "id": 1,
            "metadata": {"titles": [{"title": "   "}, {"title": " Second "}]}
        }));
        assert_eq!(record.description.title, "Second");

        let untitled = map_search(&json(serde_json::json!({
            "hits": {"hits": [{"id": 1, "metadata": {"titles": []}}]}
        })));
        assert!(matches!(
            untitled,
            Err(MappingError::MissingField {
                field: "titles",
                ..
            })
        ));
    }

    #[test]
    fn skips_an_identifier_it_cannot_normalize_rather_than_failing() {
        let record = one(serde_json::json!({
            "id": 1,
            "metadata": {
                "titles": [{"title": "T"}],
                "arxiv_eprints": [{"value": "not-an-arxiv-id"}, {"value": "2401.00001v3"}],
                "dois": [{"value": "nonsense"}, {"value": "10.1/ok"}]
            }
        }));
        assert_eq!(record.identifiers.arxiv.unwrap().as_str(), "2401.00001");
        assert_eq!(record.identifiers.doi.unwrap().as_str(), "10.1/ok");
    }

    #[test]
    fn a_record_without_an_update_timestamp_has_no_revision() {
        let record = one(serde_json::json!({
            "id": 1,
            "updated": "  ",
            "metadata": {"titles": [{"title": "T"}]}
        }));
        assert!(record.revision.is_none());
    }

    #[test]
    fn an_empty_or_malformed_response_is_distinguishable() {
        assert!(
            map_search(&json(serde_json::json!({"hits": {"hits": []}})))
                .unwrap()
                .is_empty()
        );
        assert!(map_search(&RawJson::new("not json at all")).is_err());
    }

    #[test]
    fn splitting_bibtex_validates_structure_only() {
        let entries = split_entries(&RawBibtex::new(
            "@article{A,title={One}}\n\n@article{B,title={Two}}\n",
        ))
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source_key().as_str(), "A");
        // An entry with no title is structurally fine: bibi maps description
        // from the structured record, not from this.
        assert!(split_entries(&RawBibtex::new("@article{A,doi={10.1/x}}")).is_ok());
        assert!(split_entries(&RawBibtex::new("@article{A,title={unclosed}")).is_err());
        assert!(split_entries(&RawBibtex::new("")).unwrap().is_empty());
    }
}
