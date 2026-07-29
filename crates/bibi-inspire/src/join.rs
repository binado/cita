//! Pairing bulk BibTeX back to the records that were asked for.
//!
//! This is the one place in bibi where a provider response has no envelope: a
//! BibTeX search returns a flat concatenation of entries, and nothing in the
//! format carries a record identifier. The pairing is therefore *verified*
//! rather than assumed, so the failure the hazard describes — a silent
//! mispairing that attaches one paper's BibTeX to another paper's record —
//! cannot occur. A batch is provably paired, refused whole, or short by records
//! that are individually reported.
//!
//! What makes this sound: INSPIRE declares the texkeys, bibi does not guess
//! them. The entry key is read syntactically by the scanner that already owns
//! key spans, never parsed as metadata. The join is an exact match on a
//! provider-declared unique token, with both non-injective cases rejected.

use crate::error;
use bibi_bibtex::BibtexEntry;
use bibi_core::ProviderId;
use bibi_provider::{MappingError, PayloadItem};
use std::collections::HashMap;

/// What a batch declared about one record's citation keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredKeys {
    /// The record the keys belong to.
    pub provider_id: ProviderId,
    /// Every texkey INSPIRE declares for it.
    pub texkeys: Vec<String>,
}

/// Pair entries to requested records, or refuse the batch.
///
/// Two outcomes are *ambiguity* and fail the batch whole: an entry whose texkey
/// matches no requested record, and a texkey claimed by more than one record.
/// In both, no pairing in the response can be trusted, so none is accepted.
///
/// A requested record that receives no entry is not ambiguity — nothing is
/// unclear about absence — so it comes back as `None`, is reported missing, and
/// the rest of the batch proceeds.
pub fn join(
    requested: &[ProviderId],
    declared: &[DeclaredKeys],
    entries: Vec<BibtexEntry>,
) -> Result<Vec<PayloadItem>, MappingError> {
    let mut index: HashMap<&str, &ProviderId> = HashMap::new();
    for record in declared {
        for texkey in &record.texkeys {
            if let Some(previous) = index.insert(texkey.as_str(), &record.provider_id)
                && *previous != record.provider_id
            {
                return Err(error::ambiguous_join(format!(
                    "texkey `{texkey}` is claimed by records {previous} and {}",
                    record.provider_id
                )));
            }
        }
    }

    let mut paired: HashMap<&ProviderId, BibtexEntry> = HashMap::new();
    for entry in entries {
        let key = entry.source_key().as_str().to_owned();
        let Some(owner) = index.get(key.as_str()) else {
            return Err(error::ambiguous_join(format!(
                "returned an entry keyed `{key}`, which no requested record claims"
            )));
        };
        if paired.insert(owner, entry).is_some() {
            return Err(error::ambiguous_join(format!(
                "record {owner} received more than one entry"
            )));
        }
    }

    Ok(requested
        .iter()
        .map(|provider_id| PayloadItem {
            provider_id: provider_id.clone(),
            payload: paired.remove(provider_id),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ProviderId {
        ProviderId::new(value).unwrap()
    }

    fn declared(provider_id: &str, texkeys: &[&str]) -> DeclaredKeys {
        DeclaredKeys {
            provider_id: id(provider_id),
            texkeys: texkeys.iter().map(|key| (*key).to_owned()).collect(),
        }
    }

    fn entry(texkey: &str) -> BibtexEntry {
        BibtexEntry::parse_one(format!("@article{{{texkey},title={{T}}}}")).unwrap()
    }

    #[test]
    fn a_clean_bijection_pairs_every_entry_with_its_record() {
        let requested = [id("1"), id("2")];
        let declared = [
            declared("1", &["First:2012"]),
            declared("2", &["Second:2013"]),
        ];
        let items = join(
            &requested,
            &declared,
            vec![entry("Second:2013"), entry("First:2012")],
        )
        .unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].provider_id, id("1"));
        assert_eq!(
            items[0].payload.as_ref().unwrap().source_key().as_str(),
            "First:2012"
        );
        assert_eq!(
            items[1].payload.as_ref().unwrap().source_key().as_str(),
            "Second:2013"
        );
    }

    #[test]
    fn an_alternate_texkey_still_pairs() {
        // INSPIRE declares several keys per record and may render any of them.
        let items = join(
            &[id("1")],
            &[declared("1", &["Primary:2012", "Alternate:2012"])],
            vec![entry("Alternate:2012")],
        )
        .unwrap();
        assert_eq!(
            items[0].payload.as_ref().unwrap().source_key().as_str(),
            "Alternate:2012"
        );
    }

    #[test]
    fn an_entry_matching_no_requested_record_fails_the_batch() {
        let error = join(
            &[id("1")],
            &[declared("1", &["First:2012"])],
            vec![entry("First:2012"), entry("Stranger:2020")],
        )
        .unwrap_err();
        assert!(matches!(error, MappingError::AmbiguousJoin { .. }));
        assert!(error.to_string().contains("Stranger:2020"));
    }

    #[test]
    fn a_texkey_claimed_by_two_records_fails_the_batch() {
        let error = join(
            &[id("1"), id("2")],
            &[
                declared("1", &["Shared:2012"]),
                declared("2", &["Shared:2012"]),
            ],
            vec![entry("Shared:2012")],
        )
        .unwrap_err();
        assert!(matches!(error, MappingError::AmbiguousJoin { .. }));
        assert!(error.to_string().contains("claimed by records"));
    }

    #[test]
    fn one_record_receiving_two_entries_fails_the_batch() {
        let error = join(
            &[id("1")],
            &[declared("1", &["Primary:2012", "Alternate:2012"])],
            vec![entry("Primary:2012"), entry("Alternate:2012")],
        )
        .unwrap_err();
        assert!(matches!(error, MappingError::AmbiguousJoin { .. }));
    }

    #[test]
    fn a_record_that_received_no_entry_is_absence_not_ambiguity() {
        let items = join(
            &[id("1"), id("2")],
            &[
                declared("1", &["First:2012"]),
                declared("2", &["Second:2013"]),
            ],
            vec![entry("First:2012")],
        )
        .unwrap();
        assert!(items[0].payload.is_some());
        assert!(items[1].payload.is_none(), "reported, not refused");
    }

    #[test]
    fn a_record_repeating_its_own_texkey_is_not_a_conflict() {
        let items = join(
            &[id("1")],
            &[declared("1", &["Same:2012", "Same:2012"])],
            vec![entry("Same:2012")],
        )
        .unwrap();
        assert!(items[0].payload.is_some());
    }
}
