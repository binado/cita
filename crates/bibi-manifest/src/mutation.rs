//! Pure candidate mutations.
//!
//! Each one leaves the candidate valid or leaves it untouched. None of them
//! decides policy: whether a duplicate should be skipped or overwritten, and
//! whether a failed item should stop a batch, belong to the application.

use crate::{candidate::ManifestCandidate, error::Error};
use bibi_bibtex::CitationKey;
use bibi_core::{BibiId, ProviderOwned, Record};

impl ManifestCandidate {
    /// Add a new record.
    ///
    /// Rejects a repeated bibi id, a citation key that belongs to another
    /// record, and a work already present under a shared identifier. The
    /// duplicate check is a backstop: the application decides beforehand
    /// whether a duplicate should be skipped or should overwrite, and this
    /// keeps a mistake there from producing a candidate that fails validation
    /// as a whole and discards an entire batch.
    pub fn insert(&mut self, record: Record) -> Result<(), Error> {
        if let Some(existing) = self.get(&record.id) {
            return Err(Error::Duplicate {
                kind: "record id",
                value: record.id.to_string(),
                first: existing.key.to_string(),
                second: record.key.to_string(),
            });
        }
        if let Some(existing) = self.by_key(&record.key) {
            return Err(Error::KeyInUse {
                key: existing.key.to_string(),
            });
        }
        if let Some(existing) = self.duplicate_of(&record.identifiers, &record.provenance) {
            return Err(Error::Duplicate {
                kind: "identifier",
                value: describe(&record),
                first: existing.key.to_string(),
                second: record.key.to_string(),
            });
        }
        self.records_mut().push(record);
        Ok(())
    }

    /// Replace everything a provider owns about one record.
    ///
    /// The bibi id and the local key are preserved, which is what makes this
    /// one operation serve overwrite, provider migration, and refresh alike.
    pub fn replace(&mut self, id: &BibiId, owned: ProviderOwned) -> Result<(), Error> {
        let position = self.position(id)?;
        let mut updated = self.records()[position].clone();
        updated.replace_provider_owned(owned)?;
        if let Some(existing) =
            self.duplicate_of_excluding(&updated.identifiers, &updated.provenance, id)
        {
            return Err(Error::Duplicate {
                kind: "identifier",
                value: describe(&updated),
                first: existing.key.to_string(),
                second: updated.key.to_string(),
            });
        }
        self.records_mut()[position] = updated;
        Ok(())
    }

    /// Remove one record, returning it so the command can emit what it deleted.
    pub fn remove(&mut self, id: &BibiId) -> Result<Record, Error> {
        let position = self.position(id)?;
        Ok(self.records_mut().remove(position))
    }

    /// Change one record's local citation key.
    pub fn rename(&mut self, id: &BibiId, key: CitationKey) -> Result<(), Error> {
        if let Some(existing) = self.by_key(&key)
            && existing.id != *id
        {
            return Err(Error::KeyInUse {
                key: key.to_string(),
            });
        }
        let position = self.position(id)?;
        self.records_mut()[position].rename(key)?;
        Ok(())
    }

    fn position(&self, id: &BibiId) -> Result<usize, Error> {
        self.records()
            .iter()
            .position(|record| record.id == *id)
            .ok_or_else(|| Error::UnknownRecord { id: id.to_string() })
    }
}

/// Name whichever identifier made two records the same work.
fn describe(record: &Record) -> String {
    if let Some(doi) = &record.identifiers.doi {
        return doi.to_string();
    }
    if let Some(arxiv) = &record.identifiers.arxiv {
        return arxiv.to_string();
    }
    match record.provenance.identity() {
        Some((provider, id)) => format!("{provider}:{id}"),
        None => record.key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{entry, record};
    use bibi_core::{ArxivId, Description, Identifiers, Provenance, ProviderName};

    fn candidate() -> ManifestCandidate {
        let mut candidate = ManifestCandidate::empty();
        candidate.insert(record("Alpha", "inspire", "1")).unwrap();
        candidate.insert(record("Zed", "inspire", "2")).unwrap();
        candidate
    }

    #[test]
    fn insert_rejects_a_key_that_belongs_to_another_record() {
        let mut candidate = candidate();
        assert!(matches!(
            candidate.insert(record("Alpha", "inspire", "99")),
            Err(Error::KeyInUse { .. })
        ));
        assert_eq!(candidate.records().len(), 2);
    }

    #[test]
    fn insert_rejects_a_repeated_record_id_and_names_both_records() {
        let mut candidate = candidate();
        let mut repeated = record("NewKey", "inspire", "99");
        repeated.id = candidate.records()[0].id;
        assert!(matches!(
            candidate.insert(repeated),
            Err(Error::Duplicate {
                kind: "record id",
                ref first,
                ref second,
                ..
            }) if first == "Alpha" && second == "NewKey"
        ));
    }

    #[test]
    fn insert_rejects_a_work_already_present_under_another_key() {
        let mut candidate = candidate();
        let mut duplicate = record("Different", "inspire", "1");
        duplicate.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
        assert!(matches!(
            candidate.insert(duplicate),
            Err(Error::Duplicate {
                kind: "identifier",
                ..
            })
        ));
    }

    #[test]
    fn replace_preserves_id_and_key_and_refuses_a_new_collision() {
        let mut candidate = candidate();
        let target = candidate.records()[0].id;
        let mut migrated = record("Ignored", "local", "9").provider_owned();
        migrated.provenance = Provenance::unmanaged(ProviderName::new("local").unwrap());
        migrated.payload = entry("@misc{Whatever,title={Migrated}}");
        migrated.description = Description {
            title: "Migrated".into(),
            ..Description::default()
        };
        candidate.replace(&target, migrated).unwrap();
        let updated = candidate.get(&target).unwrap();
        assert_eq!(updated.key.as_str(), "Alpha");
        assert_eq!(updated.description.title, "Migrated");
        assert_eq!(updated.provenance.provider.as_str(), "local");

        // Taking over the other record's identity is a collision, not a merge.
        let mut colliding = candidate.records()[1].provider_owned();
        colliding.identifiers = Identifiers::default();
        let other = candidate.records()[1].id;
        let mut owned = candidate.get(&target).unwrap().provider_owned();
        owned.provenance = candidate.get(&other).unwrap().provenance.clone();
        assert!(matches!(
            candidate.replace(&target, owned),
            Err(Error::Duplicate { .. })
        ));
    }

    #[test]
    fn rename_rejects_a_taken_key_but_accepts_a_records_own_key() {
        let mut candidate = candidate();
        let target = candidate.records()[0].id;
        assert!(matches!(
            candidate.rename(&target, CitationKey::new("Zed").unwrap()),
            Err(Error::KeyInUse { .. })
        ));
        candidate
            .rename(&target, CitationKey::new("Alpha").unwrap())
            .unwrap();
        candidate
            .rename(&target, CitationKey::new("Renamed").unwrap())
            .unwrap();
        assert_eq!(candidate.get(&target).unwrap().key.as_str(), "Renamed");
        assert_eq!(
            candidate.get(&target).unwrap().rendered().unwrap(),
            "@misc{Renamed,title={Alpha}}"
        );
    }

    #[test]
    fn remove_returns_the_record_and_unknown_ids_are_errors() {
        let mut candidate = candidate();
        let target = candidate.records()[0].id;
        let removed = candidate.remove(&target).unwrap();
        assert_eq!(removed.key.as_str(), "Alpha");
        assert_eq!(candidate.records().len(), 1);
        assert!(matches!(
            candidate.remove(&target),
            Err(Error::UnknownRecord { .. })
        ));
    }
}
