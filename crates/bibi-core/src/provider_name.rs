//! Provenance values: which provider owns a record, its handle, and its token.

use crate::error::Error;
use std::{fmt, str::FromStr};

/// The name of a provider, in the grammar `[a-z][a-z0-9-]*`.
///
/// A name is how a manifest refers to a provider that a given build may not
/// carry, so the grammar is fixed rather than derived from the installed set.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderName(String);

impl ProviderName {
    /// Validate and construct a provider name.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        let mut bytes = value.bytes();
        let valid = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
            && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(Error::InvalidProviderName { value })
        }
    }

    /// Borrow the name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ProviderName {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// A provider's own stable handle for a record.
///
/// Opaque: bibi compares and stores it and never parses it, in particular never
/// as a number, because the next provider's handles will not be numeric.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId(String);

/// A provider's opaque change token.
///
/// A revision is compared for equality and nothing else. A provider that cannot
/// supply a token with the required semantics leaves it absent, and its records
/// are treated as changed on every sync rather than wrongly assumed current.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Revision(String);

macro_rules! opaque_value {
    ($name:ident, $what:literal) => {
        impl $name {
            /// Validate and construct the value. Only emptiness is rejected.
            pub fn new(value: impl Into<String>) -> Result<Self, Error> {
                let value = value.into();
                if value.trim().is_empty() {
                    Err(Error::EmptyOpaqueValue { what: $what })
                } else {
                    Ok(Self(value))
                }
            }

            /// Borrow the value.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }
    };
}

opaque_value!(ProviderId, "id");
opaque_value!(Revision, "revision");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_accept_the_documented_grammar() {
        for value in ["inspire", "ads", "local", "doi-registry", "a1"] {
            assert!(ProviderName::new(value).is_ok(), "{value}");
        }
        for value in [
            "", "INSPIRE", "1inspire", "-inspire", "in spire", "in_spire", "inspiré",
        ] {
            assert!(ProviderName::new(value).is_err(), "{value}");
        }
    }

    #[test]
    fn opaque_values_reject_only_emptiness_and_keep_their_bytes() {
        assert_eq!(ProviderId::new("1124337").unwrap().as_str(), "1124337");
        assert_eq!(
            Revision::new("2026-07-27T12:34:56+00:00").unwrap().as_str(),
            "2026-07-27T12:34:56+00:00"
        );
        // Opaque means opaque: a provider may hand back anything non-empty.
        assert_eq!(
            ProviderId::new("2024ApJ...900..1X").unwrap().as_str(),
            "2024ApJ...900..1X"
        );
        assert!(ProviderId::new("").is_err());
        assert!(Revision::new("   ").is_err());
    }
}
