//! Surrogate record identity.

use crate::error::Error;
use std::{fmt, str::FromStr};
use uuid::Uuid;

/// A bibi-generated record identity.
///
/// The id is minted once and never changes: not on rename, not on refresh, not
/// on re-resolution, and not when a record migrates between providers (I5).
/// Provider ids may be replaced; this one is bibi's own, which is what lets a
/// record keep a stable identity while everything a provider owns is refetched.
///
/// It is the primary key, not a deduplication key. Two collaborators adding the
/// same work on separate branches mint different ids, so deduplication uses
/// normalized identifiers instead. It is also not an ordinary CLI selector.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BibiId(Uuid);

impl BibiId {
    /// Mint a new random identity.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Borrow the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl fmt::Display for BibiId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Canonical lowercase hyphenated form, which is the serialized shape.
        write!(formatter, "{}", self.0.as_hyphenated())
    }
}

impl FromStr for BibiId {
    type Err = Error;

    /// Parse the canonical hyphenated form only.
    ///
    /// UUID libraries accept braced, URN, and unhyphenated spellings; accepting
    /// them here would let one logical id have several manifest spellings, and
    /// the file is meant to be diffed.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::InvalidBibiId {
            value: value.to_owned(),
        };
        let uuid = Uuid::try_parse(value).map_err(|_| invalid())?;
        if uuid.as_hyphenated().to_string() != value {
            return Err(invalid());
        }
        Ok(Self(uuid))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_ids_round_trip_through_their_canonical_form() {
        let id = BibiId::new();
        let text = id.to_string();
        assert_eq!(text.len(), 36);
        assert_eq!(text, text.to_lowercase());
        assert_eq!(text.parse::<BibiId>().unwrap(), id);
    }

    #[test]
    fn minting_produces_distinct_version_four_ids() {
        let first = BibiId::new();
        let second = BibiId::new();
        assert_ne!(first, second);
        assert_eq!(first.as_uuid().get_version_num(), 4);
    }

    #[test]
    fn only_the_canonical_spelling_parses() {
        let canonical = "d760f219-9098-4b49-9f62-10cbbcc22b11";
        assert!(canonical.parse::<BibiId>().is_ok());
        for value in [
            "D760F219-9098-4B49-9F62-10CBBCC22B11",
            "{d760f219-9098-4b49-9f62-10cbbcc22b11}",
            "urn:uuid:d760f219-9098-4b49-9f62-10cbbcc22b11",
            "d760f21990984b499f6210cbbcc22b11",
            "not-a-uuid",
            "",
        ] {
            assert!(value.parse::<BibiId>().is_err(), "{value}");
        }
    }
}
