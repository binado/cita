//! Stable local record identity.

use crate::Error;
use std::{fmt, str::FromStr};
use uuid::Uuid;

/// A bibi-generated UUIDv4 that remains stable while record state changes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RecordId(Uuid);

impl RecordId {
    /// Mint a new identity for a record entering a bibliography.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Borrow the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.as_hyphenated())
    }
}

impl FromStr for RecordId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::InvalidRecordId {
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
    fn minted_ids_are_distinct_canonical_v4_uuids() {
        let first = RecordId::new();
        let second = RecordId::new();
        assert_ne!(first, second);
        assert_eq!(first.as_uuid().get_version_num(), 4);
        assert_eq!(first.to_string().parse(), Ok(first));
    }
}
