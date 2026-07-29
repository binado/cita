//! The record: six groups of fields, named for the role each plays.

use crate::{
    error::Error,
    id::BibiId,
    identifiers::{ArxivId, Doi},
    provider_name::{ProviderId, ProviderName, Revision},
};
use bibi_bibtex::{BibtexEntry, CitationKey};

/// Which provider owns a record's refresh lifecycle.
///
/// Only the name is always present. A local record has neither a provider id
/// nor a revision, and a provider that cannot supply a useful revision leaves
/// it absent — which is a different statement from "nothing changed".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provenance {
    /// The owning provider.
    pub provider: ProviderName,
    /// The provider's stable handle for this record.
    pub provider_id: Option<ProviderId>,
    /// The provider's opaque change token.
    pub revision: Option<Revision>,
}

impl Provenance {
    /// A record owned by a provider that refreshes it.
    pub fn managed(
        provider: ProviderName,
        provider_id: ProviderId,
        revision: Option<Revision>,
    ) -> Self {
        Self {
            provider,
            provider_id: Some(provider_id),
            revision,
        }
    }

    /// A record whose provider holds no handle for it.
    pub fn unmanaged(provider: ProviderName) -> Self {
        Self {
            provider,
            provider_id: None,
            revision: None,
        }
    }

    /// The `(provider, id)` pair records are deduplicated by, when there is one.
    pub fn identity(&self) -> Option<(&ProviderName, &ProviderId)> {
        self.provider_id.as_ref().map(|id| (&self.provider, id))
    }
}

/// Canonical normalized identifiers. Operational, not descriptive.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Identifiers {
    /// The canonical DOI.
    pub doi: Option<Doi>,
    /// The canonical, versionless arXiv identifier.
    pub arxiv: Option<ArxivId>,
}

impl Identifiers {
    /// True when this record and `other` name the same work by an identifier.
    pub fn intersects(&self, other: &Self) -> bool {
        (self.doi.is_some() && self.doi == other.doi)
            || (self.arxiv.is_some() && self.arxiv == other.arxiv)
    }
}

/// Advisory display data.
///
/// Description never reaches a generated artifact (I7): rendering uses the local
/// key and the payload alone. A wrong value here can produce a poor listing but
/// cannot change a deliverable, which is why it is trusted on write and repaired
/// by a refresh rather than verified against the payload.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Description {
    /// The work's title.
    pub title: String,
    /// Authors, in provider order.
    pub authors: Vec<String>,
    /// Collaborations, in provider order.
    pub collaborations: Vec<String>,
    /// The year, when the provider states one.
    pub year: Option<i32>,
}

/// One bibliography record.
///
/// Construction goes through [`Record::new`], which enforces what the rest of
/// bibi assumes: a non-empty title, a payload the local key can be written
/// into, and no revision without a provider id to refresh with. The type is
/// `#[non_exhaustive]` so that no other crate can assemble one around those
/// checks.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Record {
    /// Immutable surrogate identity.
    pub id: BibiId,
    /// The user-facing citation key.
    pub key: CitationKey,
    /// Which provider owns refresh.
    pub provenance: Provenance,
    /// Canonical identifiers.
    pub identifiers: Identifiers,
    /// Verbatim BibTeX, as received.
    pub payload: BibtexEntry,
    /// Advisory display data.
    pub description: Description,
}

/// Everything a provider owns about a record.
///
/// Grouped because these four move together: an overwrite, a provider
/// migration, and a refresh all replace exactly this much while preserving the
/// bibi id and the local key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOwned {
    /// Which provider owns refresh.
    pub provenance: Provenance,
    /// Canonical identifiers.
    pub identifiers: Identifiers,
    /// Verbatim BibTeX, as received.
    pub payload: BibtexEntry,
    /// Advisory display data.
    pub description: Description,
}

impl Record {
    /// Validate and construct a record.
    pub fn new(id: BibiId, key: CitationKey, owned: ProviderOwned) -> Result<Self, Error> {
        let record = Self {
            id,
            key,
            provenance: owned.provenance,
            identifiers: owned.identifiers,
            payload: owned.payload,
            description: owned.description,
        };
        record.validate()?;
        Ok(record)
    }

    /// Re-check every invariant a record is required to satisfy.
    ///
    /// Called on construction and again over every candidate before a write, so
    /// that a record edited in place — or introduced by a hand-merged manifest —
    /// cannot reach disk in a state the rest of bibi does not expect.
    pub fn validate(&self) -> Result<(), Error> {
        if self.description.title.trim().is_empty() {
            return Err(Error::MissingTitle {
                key: self.key.to_string(),
            });
        }
        if self.provenance.revision.is_some() && self.provenance.provider_id.is_none() {
            return Err(Error::RevisionWithoutProviderId {
                key: self.key.to_string(),
            });
        }
        self.payload
            .rekey(&self.key)
            .map_err(|source| Error::Payload {
                key: self.key.to_string(),
                source,
            })?;
        Ok(())
    }

    /// This record's payload, re-keyed to its local key.
    pub fn rendered(&self) -> Result<String, Error> {
        self.payload
            .rekey(&self.key)
            .map_err(|source| Error::Payload {
                key: self.key.to_string(),
                source,
            })
    }

    /// Change the local key, preserving everything else.
    pub fn rename(&mut self, key: CitationKey) -> Result<(), Error> {
        let previous = std::mem::replace(&mut self.key, key);
        match self.validate() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.key = previous;
                Err(error)
            }
        }
    }

    /// Replace everything the provider owns, preserving id and local key.
    ///
    /// This one operation serves overwrite, provider migration, and refresh:
    /// they differ in what produced the new values, not in what is written.
    pub fn replace_provider_owned(&mut self, owned: ProviderOwned) -> Result<(), Error> {
        let candidate = Self::new(self.id, self.key.clone(), owned)?;
        *self = candidate;
        Ok(())
    }

    /// Everything this record's provider owns, cloned out.
    pub fn provider_owned(&self) -> ProviderOwned {
        ProviderOwned {
            provenance: self.provenance.clone(),
            identifiers: self.identifiers.clone(),
            payload: self.payload.clone(),
            description: self.description.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> BibtexEntry {
        BibtexEntry::parse_one(source.to_owned()).unwrap()
    }

    fn owned() -> ProviderOwned {
        ProviderOwned {
            provenance: Provenance::managed(
                ProviderName::new("inspire").unwrap(),
                ProviderId::new("1124337").unwrap(),
                Some(Revision::new("2026-01-01").unwrap()),
            ),
            identifiers: Identifiers {
                doi: Some(Doi::new("10.1/abc").unwrap()),
                arxiv: Some(ArxivId::new("1207.7214").unwrap()),
            },
            payload: entry("@article{Provider:2012,title={T}}"),
            description: Description {
                title: "T".into(),
                ..Description::default()
            },
        }
    }

    fn record() -> Record {
        Record::new(
            BibiId::new(),
            CitationKey::new("Local:2012").unwrap(),
            owned(),
        )
        .unwrap()
    }

    #[test]
    fn a_record_renders_its_payload_under_its_local_key() {
        assert_eq!(
            record().rendered().unwrap(),
            "@article{Local:2012,title={T}}"
        );
    }

    #[test]
    fn construction_requires_a_title() {
        let untitled = ProviderOwned {
            description: Description::default(),
            ..owned()
        };
        assert!(matches!(
            Record::new(BibiId::new(), CitationKey::new("K").unwrap(), untitled),
            Err(Error::MissingTitle { .. })
        ));
    }

    #[test]
    fn construction_refuses_a_revision_without_a_provider_id() {
        let dangling = ProviderOwned {
            provenance: Provenance {
                provider: ProviderName::new("inspire").unwrap(),
                provider_id: None,
                revision: Some(Revision::new("2026-01-01").unwrap()),
            },
            ..owned()
        };
        assert!(matches!(
            Record::new(BibiId::new(), CitationKey::new("K").unwrap(), dangling),
            Err(Error::RevisionWithoutProviderId { .. })
        ));
    }

    #[test]
    fn rename_preserves_identity_and_payload_bytes() {
        let mut record = record();
        let id = record.id;
        let payload = record.payload.clone();
        record.rename(CitationKey::new("Renamed").unwrap()).unwrap();
        assert_eq!(record.id, id);
        assert_eq!(record.payload, payload);
        assert_eq!(record.rendered().unwrap(), "@article{Renamed,title={T}}");
    }

    #[test]
    fn replacing_provider_owned_data_keeps_id_and_key() {
        let mut record = record();
        let id = record.id;
        let key = record.key.clone();
        record
            .replace_provider_owned(ProviderOwned {
                payload: entry("@article{Other:2012,title={Refreshed}}"),
                description: Description {
                    title: "Refreshed".into(),
                    ..Description::default()
                },
                ..owned()
            })
            .unwrap();
        assert_eq!(record.id, id);
        assert_eq!(record.key, key);
        assert_eq!(record.description.title, "Refreshed");
        assert_eq!(
            record.rendered().unwrap(),
            "@article{Local:2012,title={Refreshed}}"
        );
    }

    #[test]
    fn identifiers_intersect_only_on_a_shared_present_value() {
        let both = Identifiers {
            doi: Some(Doi::new("10.1/a").unwrap()),
            arxiv: Some(ArxivId::new("1207.7214").unwrap()),
        };
        let shared_arxiv = Identifiers {
            doi: Some(Doi::new("10.1/b").unwrap()),
            arxiv: Some(ArxivId::new("1207.7214").unwrap()),
        };
        assert!(both.intersects(&shared_arxiv));
        assert!(!both.intersects(&Identifiers::default()));
        // Two records that share only *absence* are not the same work.
        assert!(!Identifiers::default().intersects(&Identifiers::default()));
    }
}
