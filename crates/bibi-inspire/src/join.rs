//! Verified pairing of flat INSPIRE BibTeX responses to requested records.

use crate::error;
use bibi_bibtex::BibtexEntry;
use bibi_core::{ProviderId, remote::MappingError};
use std::collections::HashMap;

/// Texkeys INSPIRE declares for one stable record id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredKeys {
    /// Stable INSPIRE id.
    pub provider_id: ProviderId,
    /// Every declared texkey.
    pub texkeys: Vec<String>,
}

/// One entry paired to its stable provider identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinedEntry {
    /// Stable INSPIRE id.
    pub provider_id: ProviderId,
    /// Exact returned BibTeX.
    pub bibtex: BibtexEntry,
}

/// Pair a complete batch or reject it whole.
pub fn join(
    requested: &[ProviderId],
    declared: &[DeclaredKeys],
    entries: Vec<BibtexEntry>,
) -> Result<Vec<JoinedEntry>, MappingError> {
    let mut owners: HashMap<&str, &ProviderId> = HashMap::new();
    for record in declared {
        for texkey in &record.texkeys {
            if let Some(previous) = owners.insert(texkey, &record.provider_id)
                && previous != &record.provider_id
            {
                return Err(error::ambiguous_join(format!(
                    "texkey `{texkey}` is claimed by records {previous} and {}",
                    record.provider_id
                )));
            }
        }
    }

    let mut paired = HashMap::new();
    for entry in entries {
        let texkey = entry.texkey().to_owned();
        let owner = owners.get(texkey.as_str()).ok_or_else(|| {
            error::ambiguous_join(format!(
                "returned an entry keyed `{texkey}`, which no requested record claims"
            ))
        })?;
        if paired.insert((*owner).clone(), entry).is_some() {
            return Err(error::ambiguous_join(format!(
                "record {owner} received more than one entry"
            )));
        }
    }

    requested
        .iter()
        .map(|provider_id| {
            paired
                .remove(provider_id)
                .map(|bibtex| JoinedEntry {
                    provider_id: provider_id.clone(),
                    bibtex,
                })
                .ok_or_else(|| {
                    error::ambiguous_join(format!("record {provider_id} returned no BibTeX entry"))
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ProviderId {
        ProviderId::new(value).unwrap()
    }

    fn entry(texkey: &str) -> BibtexEntry {
        BibtexEntry::parse_one(format!("@misc{{{texkey},title={{T}}}}")).unwrap()
    }

    #[test]
    fn clean_bijection_is_returned_in_request_order() {
        let declared = [
            DeclaredKeys {
                provider_id: id("1"),
                texkeys: vec!["First".into()],
            },
            DeclaredKeys {
                provider_id: id("2"),
                texkeys: vec!["Second".into()],
            },
        ];
        let joined = join(
            &[id("1"), id("2")],
            &declared,
            vec![entry("Second"), entry("First")],
        )
        .unwrap();
        assert_eq!(joined[0].bibtex.texkey(), "First");
        assert_eq!(joined[1].bibtex.texkey(), "Second");
    }

    #[test]
    fn ambiguity_and_absence_fail_the_whole_batch() {
        let declared = [DeclaredKeys {
            provider_id: id("1"),
            texkeys: vec!["First".into()],
        }];
        assert!(join(&[id("1")], &declared, vec![entry("Stranger")]).is_err());
        assert!(join(&[id("1")], &declared, Vec::new()).is_err());
    }
}
