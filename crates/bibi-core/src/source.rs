//! Record source and provider identity.

use crate::Error;
use std::{fmt, str::FromStr};

/// A remote provider compiled into this build.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProviderName {
    /// INSPIRE-HEP.
    #[default]
    Inspire,
}

impl ProviderName {
    /// Every installed provider.
    pub const ALL: [Self; 1] = [Self::Inspire];

    /// The stable serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inspire => "inspire",
        }
    }

    /// Installed names for diagnostics.
    pub fn installed_list() -> String {
        Self::ALL
            .into_iter()
            .map(Self::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for ProviderName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProviderName {
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

/// A provider's opaque stable handle for a record.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProviderId(String);

impl ProviderId {
    /// Construct a non-empty opaque handle.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.trim().is_empty() {
            Err(Error::EmptyProviderId)
        } else {
            Ok(Self(value))
        }
    }

    /// Borrow the exact provider value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ProviderId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Where complete replaceable record state came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Source {
    /// BibTeX supplied directly by the user.
    Local,
    /// State resolved and refreshed by a remote provider.
    Managed {
        /// Owning provider.
        provider: ProviderName,
        /// Provider's stable identity.
        id: ProviderId,
    },
}

impl Source {
    /// Construct managed source identity.
    pub fn managed(provider: ProviderName, id: ProviderId) -> Self {
        Self::Managed { provider, id }
    }

    /// True for user-supplied records.
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    /// The remote provider, when managed.
    pub const fn provider(&self) -> Option<ProviderName> {
        match self {
            Self::Local => None,
            Self::Managed { provider, .. } => Some(*provider),
        }
    }

    /// The remote stable identity, when managed.
    pub fn managed_identity(&self) -> Option<(ProviderName, &ProviderId)> {
        match self {
            Self::Local => None,
            Self::Managed { provider, id } => Some((*provider, id)),
        }
    }
}
