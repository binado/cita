//! Advisory filtering over listings.

use crate::{provenance::Provider, record::Record};

/// A best-effort listing filter.
///
/// Filtering is a convenience over description and never participates in
/// selector resolution: a filter that misses returns a poor search result,
/// whereas a selector that binds the wrong record corrupts data.
///
/// Every criterion is a conjunct, and an absent criterion matches everything.
/// Filtering preserves the caller's ordering.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordFilter {
    /// Keep records owned by this provider.
    pub provider: Option<Provider>,
    /// Keep structurally local records: those with no provider handle.
    pub local: bool,
    /// Keep records with this substring in any author, case-insensitively.
    pub author: Option<String>,
    /// Keep records with this substring in the title, case-insensitively.
    pub title: Option<String>,
    /// Keep records from this exact year.
    pub year: Option<i32>,
}

impl RecordFilter {
    /// True when every stated criterion holds.
    pub fn matches(&self, record: &Record) -> bool {
        self.provider
            .is_none_or(|provider| provider == record.provenance.provider)
            && (!self.local || record.provenance.provider_id.is_none())
            && self.author.as_ref().is_none_or(|needle| {
                record
                    .description
                    .authors
                    .iter()
                    .chain(record.description.collaborations.iter())
                    .any(|author| contains_ignore_case(author, needle))
            })
            && self
                .title
                .as_ref()
                .is_none_or(|needle| contains_ignore_case(&record.description.title, needle))
            && self
                .year
                .is_none_or(|year| record.description.year == Some(year))
    }

    /// True when no criterion is stated, so every record matches.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        id::BibiId,
        provenance::{ProviderId, Revision},
        record::{Description, Identifiers, Provenance, ProviderOwned},
    };
    use bibi_bibtex::{BibtexEntry, CitationKey};

    fn record(provider: Provider, title: &str, authors: &[&str], year: Option<i32>) -> Record {
        Record::new(
            BibiId::new(),
            CitationKey::new("K").unwrap(),
            ProviderOwned {
                provenance: Provenance::managed(
                    provider,
                    ProviderId::new("1").unwrap(),
                    Some(Revision::new("r").unwrap()),
                ),
                identifiers: Identifiers::default(),
                payload: BibtexEntry::parse_one("@misc{K,title={T}}".to_owned()).unwrap(),
                description: Description {
                    title: title.into(),
                    authors: authors.iter().map(|author| (*author).to_owned()).collect(),
                    collaborations: vec!["ATLAS".into()],
                    year,
                },
            },
        )
        .unwrap()
    }

    #[test]
    fn an_empty_filter_matches_everything() {
        let filter = RecordFilter::default();
        assert!(filter.is_empty());
        assert!(filter.matches(&record(Provider::Inspire, "T", &["Doe, Jane"], Some(2012))));
    }

    #[test]
    fn criteria_are_conjunctive_and_case_insensitive() {
        let record = record(
            Provider::Inspire,
            "Observation of a new particle",
            &["Aad, G."],
            Some(2012),
        );
        let filter = RecordFilter {
            title: Some("NEW PARTICLE".into()),
            author: Some("aad".into()),
            year: Some(2012),
            provider: Some(Provider::Inspire),
            ..RecordFilter::default()
        };
        assert!(filter.matches(&record));
        assert!(
            !RecordFilter {
                year: Some(2013),
                ..filter
            }
            .matches(&record)
        );
    }

    #[test]
    fn the_author_filter_also_searches_collaborations() {
        let record = record(Provider::Inspire, "T", &["Aad, G."], Some(2012));
        assert!(
            RecordFilter {
                author: Some("atlas".into()),
                ..RecordFilter::default()
            }
            .matches(&record)
        );
    }

    #[test]
    fn the_local_filter_uses_missing_provider_identity() {
        let mut local = record(Provider::Local, "T", &[], None);
        local.provenance = Provenance::unmanaged(Provider::Local);
        let managed = record(Provider::Inspire, "T", &[], None);
        let filter = RecordFilter {
            local: true,
            ..RecordFilter::default()
        };
        assert!(filter.matches(&local));
        assert!(!filter.matches(&managed));
    }
}
