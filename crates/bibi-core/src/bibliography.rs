//! The validated bibliography aggregate and all of its mutations.

use crate::{
    ArxivId, Doi, Error, Locator, ProviderId, ProviderName, Record, RecordFilter, RecordId,
    RecordState,
};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, Default)]
struct Indexes {
    by_id: HashMap<RecordId, usize>,
    by_texkey: HashMap<String, usize>,
    by_provider: HashMap<(ProviderName, ProviderId), usize>,
    by_doi: HashMap<Doi, usize>,
    by_arxiv: HashMap<ArxivId, usize>,
}

/// What a stable-identity collision means during admission.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CollisionPolicy {
    /// Reject every match.
    #[default]
    Reject,
    /// Replace one unambiguous resident match.
    Overwrite,
}

/// Whether admission inserted or replaced a record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionKind {
    /// A fresh UUID was minted.
    Added,
    /// Existing UUID was preserved and state replaced.
    Overwritten,
}

/// One ordered successful admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Admission {
    /// Resident UUID.
    pub id: RecordId,
    /// Resulting texkey.
    pub texkey: String,
    /// Mutation kind.
    pub kind: AdmissionKind,
}

/// One ordered successful refresh replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Replacement {
    /// Resident UUID.
    pub id: RecordId,
    /// Previous texkey.
    pub previous_texkey: String,
    /// Current texkey.
    pub texkey: String,
    /// Whether complete state changed.
    pub changed: bool,
}

/// A complete validated collection of resident records.
#[derive(Clone, Debug)]
pub struct Bibliography {
    records: Vec<Record>,
    indexes: Indexes,
}

impl Bibliography {
    /// Construct an empty bibliography.
    pub fn empty() -> Self {
        Self::validate(Vec::new()).expect("an empty bibliography is valid")
    }

    /// Rehydrate stored ids and states, validating the complete collection.
    pub fn restore(records: Vec<(RecordId, RecordState)>) -> Result<Self, Error> {
        let records = records
            .into_iter()
            .map(|(id, state)| Record::restore(id, state))
            .collect();
        Self::validate(records)
    }

    /// Resident records in derived-texkey order.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Number of resident records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Lookup by UUID.
    pub fn get(&self, id: &RecordId) -> Option<&Record> {
        self.indexes.by_id.get(id).map(|at| &self.records[*at])
    }

    /// Resolve one supported locator against resident indexes.
    pub fn resolve(&self, locator: &Locator) -> Result<&Record, Error> {
        let found = match locator {
            Locator::Texkey(key) => self.indexes.by_texkey.get(key),
            Locator::ProviderIdentity(provider, id) => {
                self.indexes.by_provider.get(&(*provider, id.clone()))
            }
            Locator::Doi(doi) => self.indexes.by_doi.get(doi),
            Locator::Arxiv(arxiv) => self.indexes.by_arxiv.get(arxiv),
            Locator::Opaque(value) => {
                return Err(Error::UnrecognizedLocator {
                    value: value.clone(),
                });
            }
        };
        found
            .map(|at| &self.records[*at])
            .ok_or_else(|| Error::NoMatch {
                locator: locator.to_string(),
            })
    }

    /// Filter without changing order.
    pub fn filter<'a>(&'a self, filter: &'a RecordFilter) -> impl Iterator<Item = &'a Record> {
        self.records
            .iter()
            .filter(move |record| filter.matches(record))
    }

    /// Admit a strict batch of complete states.
    pub fn add(
        self,
        states: Vec<RecordState>,
        policy: CollisionPolicy,
    ) -> Result<(Self, Vec<Admission>), Error> {
        for state in &states {
            state.validate()?;
        }
        let mut records = self.records.clone();
        let mut targeted = BTreeSet::new();
        let mut outcomes = Vec::with_capacity(states.len());
        for state in states {
            let matches = self.matching_ids(&state);
            let id = match matches.as_slice() {
                [] => {
                    let id = RecordId::new();
                    records.push(Record::restore(id, state));
                    outcomes.push(Admission {
                        id,
                        texkey: records.last().expect("just inserted").texkey().to_owned(),
                        kind: AdmissionKind::Added,
                    });
                    continue;
                }
                [id] if policy == CollisionPolicy::Reject => {
                    return Err(Error::ExistingRecord {
                        id: *id,
                        matched_by: self.match_description(&state, *id),
                    });
                }
                [id] => *id,
                ids => {
                    return Err(Error::DivergentIdentity { ids: join_ids(ids) });
                }
            };
            if !targeted.insert(id) {
                return Err(Error::ConflictingChanges { id });
            }
            let at = records
                .iter()
                .position(|record| record.id() == id)
                .expect("matched resident id");
            records[at].replace(state);
            outcomes.push(Admission {
                id,
                texkey: records[at].texkey().to_owned(),
                kind: AdmissionKind::Overwritten,
            });
        }
        Ok((Self::validate(records)?, outcomes))
    }

    /// Replace strict UUID-correlated state, preserving ids.
    pub fn replace(
        self,
        replacements: Vec<(RecordId, RecordState)>,
    ) -> Result<(Self, Vec<Replacement>), Error> {
        let mut records = self.records;
        let mut targeted = BTreeSet::new();
        let mut outcomes = Vec::with_capacity(replacements.len());
        for (id, state) in replacements {
            state.validate()?;
            if !targeted.insert(id) {
                return Err(Error::ConflictingChanges { id });
            }
            let record = records
                .iter_mut()
                .find(|record| record.id() == id)
                .ok_or(Error::UnknownRecord { id })?;
            let previous_texkey = record.texkey().to_owned();
            let changed = record.state() != &state;
            record.replace(state);
            outcomes.push(Replacement {
                id,
                previous_texkey,
                texkey: record.texkey().to_owned(),
                changed,
            });
        }
        Ok((Self::validate(records)?, outcomes))
    }

    /// Remove a strict locator batch resolved against the original collection.
    pub fn remove(self, locators: &[Locator]) -> Result<(Self, Vec<Record>), Error> {
        let mut ids = Vec::with_capacity(locators.len());
        let mut seen = BTreeSet::new();
        for locator in locators {
            let id = self.resolve(locator)?.id();
            if !seen.insert(id) {
                return Err(Error::DuplicateRemoval { id });
            }
            ids.push(id);
        }
        let mut records = self.records;
        let mut removed = Vec::with_capacity(ids.len());
        for id in ids {
            let at = records
                .iter()
                .position(|record| record.id() == id)
                .expect("resolved resident id");
            removed.push(records.remove(at));
        }
        Ok((Self::validate(records)?, removed))
    }

    fn matching_ids(&self, state: &RecordState) -> Vec<RecordId> {
        let mut ids = BTreeSet::new();
        if let Some((provider, id)) = state.source().managed_identity()
            && let Some(at) = self.indexes.by_provider.get(&(provider, id.clone()))
        {
            ids.insert(self.records[*at].id());
        }
        if let Some(doi) = state.identifiers().doi()
            && let Some(at) = self.indexes.by_doi.get(doi)
        {
            ids.insert(self.records[*at].id());
        }
        if let Some(arxiv) = state.identifiers().arxiv()
            && let Some(at) = self.indexes.by_arxiv.get(arxiv)
        {
            ids.insert(self.records[*at].id());
        }
        ids.into_iter().collect()
    }

    fn match_description(&self, state: &RecordState, id: RecordId) -> String {
        let record = self.get(&id).expect("matched id");
        let mut kinds = Vec::new();
        if state.source().managed_identity() == record.state().source().managed_identity()
            && state.source().managed_identity().is_some()
        {
            kinds.push("provider identity");
        }
        if state.identifiers().doi().is_some()
            && state.identifiers().doi() == record.state().identifiers().doi()
        {
            kinds.push("DOI");
        }
        if state.identifiers().arxiv().is_some()
            && state.identifiers().arxiv() == record.state().identifiers().arxiv()
        {
            kinds.push("arXiv id");
        }
        kinds.join(", ")
    }

    fn validate(mut records: Vec<Record>) -> Result<Self, Error> {
        for record in &records {
            record.state().validate()?;
        }
        records.sort_by(|left, right| left.texkey().cmp(right.texkey()));
        let indexes = Indexes::build(&records)?;
        Ok(Self { records, indexes })
    }
}

impl Indexes {
    fn build(records: &[Record]) -> Result<Self, Error> {
        let mut indexes = Self::default();
        for (at, record) in records.iter().enumerate() {
            if indexes.by_id.insert(record.id(), at).is_some() {
                return Err(Error::DuplicateRecordId { id: record.id() });
            }
            insert(
                &mut indexes.by_texkey,
                record.texkey().to_owned(),
                at,
                records,
                |first, second| Error::TexkeyInUse {
                    texkey: record.texkey().to_owned(),
                    first,
                    second,
                },
            )?;
            if let Some((provider, id)) = record.state().source().managed_identity() {
                insert_identity(
                    &mut indexes.by_provider,
                    (provider, id.clone()),
                    at,
                    records,
                    "provider identity",
                    format!("{provider}:{id}"),
                )?;
            }
            if let Some(doi) = record.state().identifiers().doi() {
                insert_identity(
                    &mut indexes.by_doi,
                    doi.clone(),
                    at,
                    records,
                    "DOI",
                    doi.to_string(),
                )?;
            }
            if let Some(arxiv) = record.state().identifiers().arxiv() {
                insert_identity(
                    &mut indexes.by_arxiv,
                    arxiv.clone(),
                    at,
                    records,
                    "arXiv id",
                    arxiv.to_string(),
                )?;
            }
        }
        Ok(indexes)
    }
}

fn insert<K: std::hash::Hash + Eq>(
    index: &mut HashMap<K, usize>,
    key: K,
    at: usize,
    records: &[Record],
    error: impl FnOnce(RecordId, RecordId) -> Error,
) -> Result<(), Error> {
    if let Some(first) = index.insert(key, at) {
        return Err(error(records[first].id(), records[at].id()));
    }
    Ok(())
}

fn insert_identity<K: std::hash::Hash + Eq>(
    index: &mut HashMap<K, usize>,
    key: K,
    at: usize,
    records: &[Record],
    kind: &'static str,
    value: String,
) -> Result<(), Error> {
    insert(index, key, at, records, |first, second| {
        Error::DuplicateIdentity {
            kind,
            value,
            first,
            second,
        }
    })
}

fn join_ids(ids: &[RecordId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bibtex, Description, Identifiers, Source};

    fn state(key: &str, provider_id: &str) -> RecordState {
        RecordState::new(
            Source::managed(ProviderName::Inspire, ProviderId::new(provider_id).unwrap()),
            Identifiers::default(),
            Description::new(key, Vec::new(), Vec::new(), None),
            Bibtex::new(format!("@misc{{{key},title={{{key}}}}}"), key),
        )
        .unwrap()
    }

    #[test]
    fn overwrite_preserves_uuid_while_texkey_changes() {
        let (bibliography, added) = Bibliography::empty()
            .add(vec![state("Old", "1")], CollisionPolicy::Reject)
            .unwrap();
        let id = added[0].id;
        let (bibliography, overwritten) = bibliography
            .add(vec![state("New", "1")], CollisionPolicy::Overwrite)
            .unwrap();
        assert_eq!(overwritten[0].id, id);
        assert_eq!(bibliography.records()[0].texkey(), "New");
    }

    #[test]
    fn texkey_never_selects_an_overwrite_target() {
        let (bibliography, _) = Bibliography::empty()
            .add(vec![state("Same", "1")], CollisionPolicy::Reject)
            .unwrap();
        assert!(matches!(
            bibliography.add(vec![state("Same", "2")], CollisionPolicy::Overwrite),
            Err(Error::TexkeyInUse { .. })
        ));
    }

    #[test]
    fn replacement_is_all_or_nothing_and_rejects_duplicate_targets() {
        let (bibliography, added) = Bibliography::empty()
            .add(vec![state("A", "1")], CollisionPolicy::Reject)
            .unwrap();
        let id = added[0].id;
        assert!(matches!(
            bibliography.replace(vec![(id, state("B", "1")), (id, state("C", "1"))]),
            Err(Error::ConflictingChanges { .. })
        ));
    }
}
