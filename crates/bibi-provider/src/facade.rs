//! Exhaustive dispatch over providers compiled into this build.

use bibi_core::{
    Locator, ProviderName, RecordState,
    remote::{Provider, ProviderError, RetrievalError},
};
use bibi_inspire::{InspireProvider, RetryEvent, RetryObserver, Transport};
use std::{fmt, sync::Arc};
use thiserror::Error as ThisError;

/// Selection, construction, or provider-contract failure.
#[derive(Debug, ThisError)]
pub enum Error {
    /// The requested provider is not installed.
    #[error(
        "provider `{name}` is not available in this build; installed providers: {}",
        ProviderName::installed_list()
    )]
    UnknownProvider {
        /// Supplied name.
        name: String,
    },
    /// A qualified locator conflicts with `--provider`.
    #[error("`{locator}` names `{qualified}`, but `--provider {requested}` was given")]
    ProviderConflict {
        /// Qualified locator.
        locator: String,
        /// Qualifier value.
        qualified: ProviderName,
        /// Explicit value.
        requested: ProviderName,
    },
    /// One invocation names more than one provider.
    #[error("one invocation must use one provider, but `{first}` and `{second}` were named")]
    MixedProviders {
        /// First provider.
        first: ProviderName,
        /// Conflicting provider.
        second: ProviderName,
    },
    /// Provider construction failed.
    #[error(transparent)]
    Construction(#[from] RetrievalError),
    /// Provider operation failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

enum Backend {
    Inspire(InspireProvider),
    Scripted(Arc<bibi_core::remote::testing::FakeProvider>),
}

impl Provider for Backend {
    fn name(&self) -> ProviderName {
        match self {
            Self::Inspire(provider) => provider.name(),
            Self::Scripted(provider) => provider.name(),
        }
    }

    async fn resolve(&self, locators: &[Locator]) -> Result<Vec<RecordState>, ProviderError> {
        match self {
            Self::Inspire(provider) => provider.resolve(locators).await,
            Self::Scripted(provider) => provider.resolve(locators).await,
        }
    }
}

/// Closed provider facade.
pub struct Providers {
    backend: Backend,
}

impl fmt::Debug for Providers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Providers")
            .field("installed", &ProviderName::ALL)
            .finish()
    }
}

impl Providers {
    /// Start production construction.
    pub fn builder() -> ProvidersBuilder {
        ProvidersBuilder::default()
    }

    /// Construct with public production settings.
    pub fn new() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// Parse an exact installed provider name.
    pub fn installed(value: &str) -> Result<ProviderName, Error> {
        value.parse().map_err(|_| Error::UnknownProvider {
            name: value.to_owned(),
        })
    }

    /// Resolve one strict batch through exactly one selected provider.
    pub async fn resolve(
        &self,
        selected: Option<ProviderName>,
        locators: &[Locator],
    ) -> Result<Vec<RecordState>, Error> {
        let provider = preflight(selected, locators)?;
        debug_assert_eq!(provider, self.backend.name());
        let states = self.backend.resolve(locators).await?;
        if states.len() != locators.len() {
            return Err(ProviderError::contract(
                provider,
                format!(
                    "returned {} states for {} locators",
                    states.len(),
                    locators.len()
                ),
            )
            .into());
        }
        for state in &states {
            if state.source().provider() != Some(provider) {
                return Err(ProviderError::contract(
                    provider,
                    format!("returned state with source `{:?}`", state.source()),
                )
                .into());
            }
            state
                .validate()
                .map_err(|error| ProviderError::contract(provider, error.to_string()))?;
        }
        Ok(states)
    }

    pub(crate) fn scripted(remote: Arc<bibi_core::remote::testing::FakeProvider>) -> Self {
        Self {
            backend: Backend::Scripted(remote),
        }
    }
}

/// Production construction options.
#[derive(Default)]
pub struct ProvidersBuilder {
    inspire_base_url: Option<String>,
    retry_observer: Option<RetryObserver>,
}

impl ProvidersBuilder {
    /// Override the INSPIRE base URL.
    pub fn inspire_base_url(mut self, value: impl Into<String>) -> Self {
        self.inspire_base_url = Some(value.into());
        self
    }

    /// Observe retry events.
    pub fn on_retry(mut self, observer: impl Fn(&RetryEvent) + Send + Sync + 'static) -> Self {
        self.retry_observer = Some(Arc::new(observer));
        self
    }

    /// Build the facade.
    pub fn build(self) -> Result<Providers, Error> {
        let mut transport = Transport::builder();
        if let Some(base_url) = self.inspire_base_url {
            transport = transport.base_url(base_url);
        }
        if let Some(observer) = self.retry_observer {
            transport = transport.on_retry(move |event| observer(event));
        }
        Ok(Providers {
            backend: Backend::Inspire(InspireProvider::with_transport(transport.build()?)),
        })
    }
}

fn preflight(selected: Option<ProviderName>, locators: &[Locator]) -> Result<ProviderName, Error> {
    let mut qualified = None;
    for locator in locators {
        let Locator::ProviderIdentity(provider, _) = locator else {
            continue;
        };
        if selected.is_some_and(|requested| requested != *provider) {
            return Err(Error::ProviderConflict {
                locator: locator.to_string(),
                qualified: *provider,
                requested: selected.expect("checked some"),
            });
        }
        if qualified.is_some_and(|first| first != *provider) {
            return Err(Error::MixedProviders {
                first: qualified.expect("checked some"),
                second: *provider,
            });
        }
        qualified = Some(*provider);
    }
    Ok(selected.or(qualified).unwrap_or_default())
}
