//! Provenance: which provider owns a record, its handle, its token, and the
//! [`Provenance`] the three of them compose.

use crate::error::Error;
use std::{fmt, str::FromStr};

/// A provider this build carries.
///
/// Closed: the set is fixed at compile time, so a name outside it is rejected
/// wherever it appears — a manifest field, a `--provider` flag, or a
/// `<provider>:<id>` qualifier. Kept here rather than in `bibi-provider`
/// because `bibi-manifest` and the locator parsers must name a provider
/// without depending on a network crate.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Provider {
    /// INSPIRE-HEP, the remote provider.
    #[default]
    Inspire,
    /// The local provider: user-supplied BibTeX, no refresh.
    Local,
}

impl Provider {
    /// Every provider this build carries.
    pub const ALL: [Self; 2] = [Self::Inspire, Self::Local];

    /// The stable name written into record provenance and manifests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspire => "inspire",
            Self::Local => "local",
        }
    }

    /// Whether this provider has a remote implementation, i.e. supports refresh.
    pub const fn is_remote(self) -> bool {
        matches!(self, Self::Inspire)
    }

    /// Every installed provider's name, comma-joined, for a "what would work" hint.
    pub fn installed_list() -> String {
        Self::ALL
            .into_iter()
            .map(Provider::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Provider {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.as_str() == value)
            .ok_or_else(|| Error::UnknownProvider {
                value: value.to_owned(),
            })
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

/// Which provider owns a record's refresh lifecycle.
///
/// Only the provider is always present. A local record has neither a provider
/// id nor a revision, and a provider that cannot supply a useful revision
/// leaves it absent — which is a different statement from "nothing changed".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Provenance {
    /// The owning provider.
    pub provider: Provider,
    /// The provider's stable handle for this record.
    pub provider_id: Option<ProviderId>,
    /// The provider's opaque change token.
    pub revision: Option<Revision>,
}

impl Provenance {
    /// A record owned by a provider that refreshes it.
    pub fn managed(
        provider: Provider,
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
    pub fn unmanaged(provider: Provider) -> Self {
        Self {
            provider,
            provider_id: None,
            revision: None,
        }
    }

    /// The `(provider, id)` pair records are deduplicated by, when there is one.
    pub fn identity(&self) -> Option<(Provider, &ProviderId)> {
        self.provider_id.as_ref().map(|id| (self.provider, id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_parses_only_the_installed_set() {
        assert_eq!("inspire".parse(), Ok(Provider::Inspire));
        assert_eq!("local".parse(), Ok(Provider::Local));
        for value in ["", "INSPIRE", "ads", "inspire ", " local"] {
            assert!(value.parse::<Provider>().is_err(), "{value}");
        }
    }

    #[test]
    fn provider_display_round_trips_through_from_str() {
        for provider in Provider::ALL {
            assert_eq!(provider.to_string().parse::<Provider>().unwrap(), provider);
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

    #[test]
    fn only_a_provider_handle_yields_a_deduplication_identity() {
        let id = ProviderId::new("1124337").unwrap();
        let managed = Provenance::managed(Provider::Inspire, id.clone(), None);
        assert_eq!(managed.identity(), Some((Provider::Inspire, &id)));
        // Local provenance is structurally the absence of a handle, so it
        // offers no identity to deduplicate two local records against.
        assert_eq!(Provenance::unmanaged(Provider::Local).identity(), None);
    }
}
