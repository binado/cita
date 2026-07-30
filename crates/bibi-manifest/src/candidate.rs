//! The validated manifest and the candidate that becomes one.

use crate::{error::Error, indexes::Indexes, schema};
use bibi_bibtex::CitationKey;
use bibi_core::{BibiId, Identifiers, Provenance, Record, RecordFilter, Selector, SelectorForm};

/// A validated manifest: sorted records plus their identity indexes.
///
/// Every path that produces a `Manifest` has already checked the whole record
/// list, so holding one is evidence that the invariants hold — not a promise
/// that they will be checked later.
#[derive(Debug)]
pub struct Manifest {
    records: Vec<Record>,
    indexes: Indexes,
}

impl Manifest {
    /// Validate a candidate's records into a manifest.
    pub(crate) fn validate(mut records: Vec<Record>) -> Result<Self, Error> {
        for record in &records {
            record.validate()?;
        }
        // Sorting before indexing means positions are the file's own order, so
        // a rendered manifest and an in-memory listing cannot disagree.
        records.sort_by(|left, right| left.key.cmp(&right.key));
        let indexes = Indexes::build(&records)?;
        Ok(Self { records, indexes })
    }

    /// The records, in local-key order.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// How many records the manifest holds.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True when the manifest holds no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Look a record up by its bibi id.
    pub fn get(&self, id: &BibiId) -> Option<&Record> {
        self.indexes.by_id.get(id).map(|at| &self.records[*at])
    }

    /// Look a record up by its exact local key.
    pub fn by_key(&self, key: &CitationKey) -> Option<&Record> {
        self.indexes.by_key.get(key).map(|at| &self.records[*at])
    }

    /// Resolve a selector to exactly one record.
    ///
    /// Forms are tried in the documented order and the first hit wins, so the
    /// answer depends on what the manifest holds rather than on how the user
    /// happened to spell the selector.
    pub fn resolve(&self, selector: &Selector) -> Result<&Record, Error> {
        for form in selector.forms() {
            let found = match form {
                SelectorForm::Key(key) => self.indexes.by_key.get(key),
                SelectorForm::ProviderIdentity(provider, id) => self
                    .indexes
                    .by_provider_identity
                    .get(&(*provider, id.clone())),
                SelectorForm::Doi(doi) => self.indexes.by_doi.get(doi),
                SelectorForm::Arxiv(arxiv) => self.indexes.by_arxiv.get(arxiv),
            };
            if let Some(at) = found {
                return Ok(&self.records[*at]);
            }
        }
        Err(Error::NoMatch {
            selector: selector.as_str().to_owned(),
        })
    }

    /// Find the record that already represents this work, if any.
    pub fn duplicate_of(
        &self,
        identifiers: &Identifiers,
        provenance: &Provenance,
    ) -> Option<&Record> {
        duplicate_position(&self.records, identifiers, provenance, None).map(|at| &self.records[at])
    }

    /// The records matching a filter, in local-key order.
    pub fn filter<'a>(&'a self, filter: &'a RecordFilter) -> impl Iterator<Item = &'a Record> {
        self.records
            .iter()
            .filter(move |record| filter.matches(record))
    }

    /// Serialize the manifest in its canonical schema-1 form.
    pub fn to_toml(&self) -> Result<String, Error> {
        schema::render(&self.records)
    }

    /// Reopen this manifest for mutation.
    pub fn into_candidate(self) -> ManifestCandidate {
        ManifestCandidate {
            records: self.records,
        }
    }

    /// Copy this manifest into a candidate, leaving the original intact.
    pub fn to_candidate(&self) -> ManifestCandidate {
        ManifestCandidate {
            records: self.records.clone(),
        }
    }
}

/// A manifest under construction.
///
/// A candidate is deliberately index-free. Every mutation would invalidate
/// stored positions, and the record counts bibi works at — hundreds per project,
/// ten thousand for a large personal library — make a scan cheaper than the
/// bookkeeping. Indexes are built once, when the candidate is validated into a
/// [`Manifest`], which is also where duplicates introduced by a merge surface.
#[derive(Clone, Debug, Default)]
pub struct ManifestCandidate {
    records: Vec<Record>,
}

impl ManifestCandidate {
    /// An empty candidate, as a first `add` or an `init` produces.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The records, in insertion order.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// True when the candidate holds no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Find a record by its exact local key.
    pub fn by_key(&self, key: &CitationKey) -> Option<&Record> {
        self.records.iter().find(|record| record.key == *key)
    }

    /// Find a record by its bibi id.
    pub fn get(&self, id: &BibiId) -> Option<&Record> {
        self.records.iter().find(|record| record.id == *id)
    }

    /// Find the record that already represents this work, if any.
    ///
    /// Checked against the *growing* candidate rather than only the manifest as
    /// loaded, so two entries in one import that duplicate each other fail one
    /// item instead of failing whole-candidate validation and discarding the
    /// batch.
    pub fn duplicate_of(
        &self,
        identifiers: &Identifiers,
        provenance: &Provenance,
    ) -> Option<&Record> {
        duplicate_position(&self.records, identifiers, provenance, None).map(|at| &self.records[at])
    }

    /// Find every record that already represents this work.
    ///
    /// Most callers only need to know whether any duplicate exists. Planning an
    /// overwrite is different: two identifiers may point at two different
    /// records, and choosing either would silently discard the other target.
    pub fn duplicates_of<'a>(
        &'a self,
        identifiers: &'a Identifiers,
        provenance: &'a Provenance,
    ) -> impl Iterator<Item = &'a Record> {
        self.records
            .iter()
            .filter(move |record| duplicates(record, identifiers, provenance))
    }

    /// The same query, ignoring one record — the one about to be replaced.
    pub fn duplicate_of_excluding(
        &self,
        identifiers: &Identifiers,
        provenance: &Provenance,
        exclude: &BibiId,
    ) -> Option<&Record> {
        duplicate_position(&self.records, identifiers, provenance, Some(exclude))
            .map(|at| &self.records[at])
    }

    pub(crate) fn records_mut(&mut self) -> &mut Vec<Record> {
        &mut self.records
    }

    /// Validate the complete candidate, producing a manifest.
    pub fn validate(self) -> Result<Manifest, Error> {
        Manifest::validate(self.records)
    }
}

/// Deduplication: DOI, arXiv id, and provider identity, in that order.
fn duplicate_position(
    records: &[Record],
    identifiers: &Identifiers,
    provenance: &Provenance,
    exclude: Option<&BibiId>,
) -> Option<usize> {
    records.iter().position(|record| {
        exclude.is_none_or(|id| record.id != *id) && duplicates(record, identifiers, provenance)
    })
}

fn duplicates(record: &Record, identifiers: &Identifiers, provenance: &Provenance) -> bool {
    record.identifiers.intersects(identifiers)
        || (provenance.identity().is_some()
            && record.provenance.identity() == provenance.identity())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::record;
    use bibi_core::{ArxivId, Doi, Provider};

    #[test]
    fn validation_sorts_records_by_local_key() {
        let manifest = Manifest::validate(vec![
            record("Zed", Provider::Inspire, "2"),
            record("Alpha", Provider::Inspire, "1"),
        ])
        .unwrap();
        assert_eq!(
            manifest
                .records()
                .iter()
                .map(|record| record.key.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Zed"]
        );
    }

    #[test]
    fn validation_rejects_every_duplicate_index() {
        let first = record("A", Provider::Inspire, "1");
        let mut same_key = record("A", Provider::Inspire, "2");
        same_key.description.title = "Other".into();
        assert!(matches!(
            Manifest::validate(vec![first.clone(), same_key]),
            Err(Error::Duplicate {
                kind: "citation key",
                ..
            })
        ));
        assert!(matches!(
            Manifest::validate(vec![first.clone(), record("B", Provider::Inspire, "1")]),
            Err(Error::Duplicate {
                kind: "provider identity",
                ..
            })
        ));
        let mut same_id = record("B", Provider::Inspire, "2");
        same_id.id = first.id;
        assert!(matches!(
            Manifest::validate(vec![first.clone(), same_id]),
            Err(Error::Duplicate {
                kind: "record id",
                ..
            })
        ));
        let doi = Some(Doi::new("10.1/a").unwrap());
        let mut left = record("C", Provider::Inspire, "3");
        left.identifiers.doi = doi.clone();
        let mut right = record("D", Provider::Inspire, "4");
        right.identifiers.doi = doi;
        assert!(matches!(
            Manifest::validate(vec![left, right]),
            Err(Error::Duplicate { kind: "DOI", .. })
        ));
    }

    #[test]
    fn selector_resolution_prefers_an_exact_key() {
        let mut keyed = record("2401.00001", Provider::Inspire, "1");
        keyed.identifiers.arxiv = None;
        let mut by_arxiv = record("Other", Provider::Inspire, "2");
        by_arxiv.identifiers.arxiv = Some(ArxivId::new("2401.00001").unwrap());
        let manifest = Manifest::validate(vec![keyed, by_arxiv]).unwrap();
        let resolved = manifest
            .resolve(&Selector::parse("2401.00001").unwrap())
            .unwrap();
        assert_eq!(resolved.key.as_str(), "2401.00001");
    }

    #[test]
    fn selector_resolution_falls_through_to_identifiers() {
        let mut record = record("Key", Provider::Inspire, "1124337");
        record.identifiers.doi = Some(Doi::new("10.1/abc").unwrap());
        record.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
        let manifest = Manifest::validate(vec![record]).unwrap();
        for selector in ["Key", "inspire:1124337", "10.1/abc", "1207.7214v2"] {
            assert_eq!(
                manifest
                    .resolve(&Selector::parse(selector).unwrap())
                    .unwrap()
                    .key
                    .as_str(),
                "Key",
                "{selector}"
            );
        }
        assert!(matches!(
            manifest.resolve(&Selector::parse("Missing").unwrap()),
            Err(Error::NoMatch { .. })
        ));
    }

    #[test]
    fn duplicate_detection_uses_identifiers_and_provider_identity() {
        let mut stored = record("Key", Provider::Inspire, "1124337");
        stored.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
        let mut candidate = ManifestCandidate::empty();
        candidate.insert(stored).unwrap();
        let by_arxiv = Identifiers {
            doi: None,
            arxiv: Some(ArxivId::new("1207.7214v3").unwrap()),
        };
        let unrelated = Provenance::unmanaged(Provider::Local);
        assert!(candidate.duplicate_of(&by_arxiv, &unrelated).is_some());
        let by_identity = record("Other", Provider::Inspire, "1124337");
        assert!(
            candidate
                .duplicate_of(&Identifiers::default(), &by_identity.provenance)
                .is_some()
        );
        assert!(
            candidate
                .duplicate_of(&Identifiers::default(), &unrelated)
                .is_none()
        );
    }

    #[test]
    fn duplicate_detection_can_return_every_possible_target() {
        let mut by_doi = record("ByDoi", Provider::Local, "unused");
        by_doi.provenance = Provenance::unmanaged(Provider::Local);
        by_doi.identifiers.doi = Some(Doi::new("10.1/shared").unwrap());
        let mut by_arxiv = record("ByArxiv", Provider::Local, "unused");
        by_arxiv.provenance = Provenance::unmanaged(Provider::Local);
        by_arxiv.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
        let mut candidate = ManifestCandidate::empty();
        candidate.insert(by_doi).unwrap();
        candidate.insert(by_arxiv).unwrap();

        let identifiers = Identifiers {
            doi: Some(Doi::new("10.1/shared").unwrap()),
            arxiv: Some(ArxivId::new("1207.7214v2").unwrap()),
        };
        let provenance = Provenance::unmanaged(Provider::Inspire);
        let matches = candidate
            .duplicates_of(&identifiers, &provenance)
            .map(|record| record.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(matches, ["ByDoi", "ByArxiv"]);
    }
}
