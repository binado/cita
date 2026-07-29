//! The ordered roster, and the rules for choosing who answers.

use crate::{
    contract::Provider,
    error::{MappingError, ProviderError},
    outcome::{PayloadItem, ProviderRecord, RefreshItem, RefreshRequest, Resolution},
};
use bibi_core::{Locator, ProviderId, ProviderName, QualifiedLocator};
use std::{collections::BTreeSet, sync::Arc};
use thiserror::Error as ThisError;

/// A usage error detected before any request is made.
#[derive(Debug, ThisError)]
pub enum RegistryError {
    /// A provider was named that this build does not carry.
    #[error("provider `{name}` is not available in this build")]
    UnknownProvider {
        /// The name that was asked for.
        name: ProviderName,
    },
    /// A qualified locator and `--provider` name different providers.
    #[error("`{locator}` names provider `{qualified}`, but `--provider {requested}` was given")]
    ProviderConflict {
        /// The locator as written.
        locator: String,
        /// The provider its qualifier names.
        qualified: ProviderName,
        /// The provider the flag names.
        requested: ProviderName,
    },
}

/// What resolving one locator produced, after fallback.
#[derive(Debug)]
pub enum LocatorOutcome {
    /// A provider resolved it.
    Found {
        /// Which provider answered.
        provider: ProviderName,
        /// The mapped record.
        record: Box<ProviderRecord>,
    },
    /// Every applicable provider reported absence.
    ///
    /// This is the only outcome that permits a local record to be created from
    /// a user's own entry, because it is the only one that says anything about
    /// whether the work exists.
    NotFound,
    /// No installed provider recognizes this bare id's syntax.
    Unrecognized,
    /// More than one provider claims this bare id's syntax.
    Ambiguous {
        /// The providers that claimed it.
        providers: Vec<ProviderName>,
    },
    /// A provider failed. Fallback stops here.
    Failed {
        /// Which provider failed.
        provider: ProviderName,
        /// Why.
        error: ProviderError,
    },
}

/// The installed providers, in roster order.
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn Provider>>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderRegistry")
            .field("providers", &self.names())
            .finish()
    }
}

impl ProviderRegistry {
    /// Build a registry. The order given is the roster order.
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { providers }
    }

    /// The installed provider names, in roster order.
    pub fn names(&self) -> Vec<ProviderName> {
        self.providers
            .iter()
            .map(|provider| provider.name().clone())
            .collect()
    }

    /// Look up an installed provider by name.
    pub fn get(&self, name: &ProviderName) -> Option<&Arc<dyn Provider>> {
        self.providers
            .iter()
            .find(|provider| provider.name() == name)
    }

    /// Look up an installed provider, or fail naming it.
    pub fn require(&self, name: &ProviderName) -> Result<&Arc<dyn Provider>, RegistryError> {
        self.get(name)
            .ok_or_else(|| RegistryError::UnknownProvider { name: name.clone() })
    }

    /// The names of providers that ingest records they will never refresh.
    ///
    /// This is what the `--local` listing filter is built from, so that no
    /// command has to compare a stored provider name with a literal string.
    pub fn unrefreshable_names(&self) -> Vec<ProviderName> {
        self.providers
            .iter()
            .filter(|provider| provider.capabilities().ingest_without_refresh())
            .map(|provider| provider.name().clone())
            .collect()
    }

    /// The provider that ingests user-supplied BibTeX, if one is installed.
    pub fn ingest_provider(&self) -> Option<&Arc<dyn Provider>> {
        self.providers
            .iter()
            .find(|provider| provider.capabilities().ingest)
    }

    /// Check the flag and qualifier combination before any request is made.
    pub fn preflight(
        &self,
        locators: &[QualifiedLocator],
        constraint: Option<&ProviderName>,
    ) -> Result<(), RegistryError> {
        if let Some(name) = constraint {
            self.require(name)?;
        }
        for locator in locators {
            let Some(qualified) = &locator.provider else {
                continue;
            };
            self.require(qualified)?;
            if let Some(requested) = constraint
                && qualified != requested
            {
                return Err(RegistryError::ProviderConflict {
                    locator: locator.to_string(),
                    qualified: qualified.clone(),
                    requested: requested.clone(),
                });
            }
        }
        Ok(())
    }

    /// Resolve a set of locators, returning one outcome per input, in order.
    ///
    /// Unconstrained resolution offers the whole unresolved set to the first
    /// provider in roster order, then offers the subset that came back absent
    /// to the next, and so on: absence advances, but it advances in groups, so
    /// a file of three hundred entries costs a handful of requests rather than
    /// three hundred. A retrieval or mapping error stops the locators in that
    /// call — it says nothing about whether the work exists, and letting the
    /// next provider answer would let a timeout decide permanent provenance.
    pub async fn resolve(
        &self,
        locators: &[QualifiedLocator],
        constraint: Option<&ProviderName>,
    ) -> Result<Vec<LocatorOutcome>, RegistryError> {
        self.preflight(locators, constraint)?;
        let mut outcomes: Vec<Option<LocatorOutcome>> =
            locators.iter().map(|_| None).collect::<Vec<_>>();
        let eligible = locators
            .iter()
            .enumerate()
            .map(|(at, locator)| {
                let (candidates, decided) = self.candidates(locator, constraint);
                if let Some(outcome) = decided {
                    outcomes[at] = Some(outcome);
                }
                candidates
            })
            .collect::<Vec<_>>();

        for (index, provider) in self.providers.iter().enumerate() {
            let batch = (0..locators.len())
                .filter(|at| outcomes[*at].is_none() && eligible[*at].contains(&index))
                .collect::<Vec<_>>();
            if batch.is_empty() {
                continue;
            }
            let requested = batch
                .iter()
                .map(|at| locators[*at].locator.clone())
                .collect::<Vec<_>>();
            match provider.resolve(&requested).await {
                Err(error) => {
                    // One failure fails every locator in that call, and no
                    // later provider is offered them. The first keeps the
                    // original error; the rest carry its text, since transport
                    // internals cannot be cloned.
                    let retrieval = matches!(error, ProviderError::Retrieval(_));
                    let rendered = error.to_string();
                    let mut original = Some(error);
                    for at in &batch {
                        let failure = original
                            .take()
                            .unwrap_or_else(|| restated(provider.name(), &rendered, retrieval));
                        outcomes[*at] = Some(LocatorOutcome::Failed {
                            provider: provider.name().clone(),
                            error: failure,
                        });
                    }
                }
                Ok(resolutions) => {
                    if resolutions.len() != batch.len() {
                        let message = format!(
                            "returned {} resolutions for {} locators",
                            resolutions.len(),
                            batch.len()
                        );
                        for at in &batch {
                            outcomes[*at] = Some(LocatorOutcome::Failed {
                                provider: provider.name().clone(),
                                error: ProviderError::contract(provider.name(), message.clone()),
                            });
                        }
                        continue;
                    }
                    for (at, resolution) in batch.iter().zip(resolutions) {
                        match resolution {
                            // Absence leaves the locator pending for the next
                            // provider in roster order.
                            Resolution::NotFound | Resolution::UnsupportedLocator => {}
                            Resolution::Found(record) => {
                                outcomes[*at] = Some(match validate(provider, &record) {
                                    Ok(()) => LocatorOutcome::Found {
                                        provider: provider.name().clone(),
                                        record,
                                    },
                                    Err(error) => LocatorOutcome::Failed {
                                        provider: provider.name().clone(),
                                        error,
                                    },
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(outcomes
            .into_iter()
            .map(|outcome| outcome.unwrap_or(LocatorOutcome::NotFound))
            .collect())
    }

    /// Ask one provider to examine records, validating the result's arity.
    pub async fn refresh_metadata(
        &self,
        provider: &Arc<dyn Provider>,
        requests: &[RefreshRequest],
    ) -> Result<Vec<RefreshItem>, ProviderError> {
        let items = provider.refresh_metadata(requests).await;
        let expected = requests
            .iter()
            .map(|request| request.bibi_id)
            .collect::<BTreeSet<_>>();
        let returned = items
            .iter()
            .map(|item| item.bibi_id)
            .collect::<BTreeSet<_>>();
        if items.len() != requests.len() || returned != expected {
            // Every managed record and every returned result must be
            // explained; anything else means a caller could read one record's
            // outcome as another's.
            return Err(ProviderError::contract(
                provider.name(),
                format!(
                    "returned {} refresh results for {} requests",
                    items.len(),
                    requests.len()
                ),
            ));
        }
        Ok(items)
    }

    /// Ask one provider for payloads, validating the result's arity.
    pub async fn fetch_payloads(
        &self,
        provider: &Arc<dyn Provider>,
        provider_ids: &[ProviderId],
    ) -> Result<Vec<PayloadItem>, ProviderError> {
        let items = provider.fetch_payloads(provider_ids).await?;
        let expected = provider_ids.iter().cloned().collect::<BTreeSet<_>>();
        let returned = items
            .iter()
            .map(|item| item.provider_id.clone())
            .collect::<BTreeSet<_>>();
        if items.len() != provider_ids.len() || returned != expected {
            return Err(ProviderError::contract(
                provider.name(),
                format!(
                    "returned {} payload results for {} requested ids",
                    items.len(),
                    provider_ids.len()
                ),
            ));
        }
        Ok(items)
    }

    /// Which providers may be offered this locator, or an outcome if none can.
    fn candidates(
        &self,
        locator: &QualifiedLocator,
        constraint: Option<&ProviderName>,
    ) -> (Vec<usize>, Option<LocatorOutcome>) {
        let named = locator.provider.as_ref().or(constraint);
        if let Some(name) = named {
            let at = self
                .providers
                .iter()
                .position(|provider| provider.name() == name);
            return (at.into_iter().collect(), None);
        }
        if locator.needs_provider_recognition() {
            let Locator::ProviderId(value) = &locator.locator else {
                unreachable!("only a bare provider id needs recognition");
            };
            let claimants = self
                .providers
                .iter()
                .enumerate()
                .filter(|(_, provider)| provider.recognizes_unqualified_id(value))
                .collect::<Vec<_>>();
            return match claimants.len() {
                0 => (Vec::new(), Some(LocatorOutcome::Unrecognized)),
                1 => (vec![claimants[0].0], None),
                _ => (
                    Vec::new(),
                    Some(LocatorOutcome::Ambiguous {
                        providers: claimants
                            .into_iter()
                            .map(|(_, provider)| provider.name().clone())
                            .collect(),
                    }),
                ),
            };
        }
        (
            self.providers
                .iter()
                .enumerate()
                .filter(|(_, provider)| provider.capabilities().supports(&locator.locator))
                .map(|(at, _)| at)
                .collect(),
            None,
        )
    }
}

/// Check the promises the registry makes on every provider's behalf.
fn validate(provider: &Arc<dyn Provider>, record: &ProviderRecord) -> Result<(), ProviderError> {
    let name = provider.name();
    if record.provenance.provider != *name {
        return Err(ProviderError::contract(
            name,
            format!(
                "returned a record owned by `{}`",
                record.provenance.provider
            ),
        ));
    }
    if provider.capabilities().refresh && record.provenance.provider_id.is_none() {
        return Err(ProviderError::contract(
            name,
            "refreshes its records but returned one without a provider id",
        ));
    }
    if record.provenance.revision.is_some() && record.provenance.provider_id.is_none() {
        return Err(ProviderError::contract(
            name,
            "returned a revision without a provider id",
        ));
    }
    Ok(())
}

/// Restate a failure for the other locators the same failed call carried.
///
/// `ProviderError` wraps source errors and is deliberately not `Clone`, so
/// every locator after the first reports the same failure through its rendered
/// text. The retrieval/mapping split is preserved, because that is what decides
/// whether retrying could help.
fn restated(provider: &ProviderName, message: &str, retrieval: bool) -> ProviderError {
    if retrieval {
        ProviderError::Retrieval(crate::error::RetrievalError::Transport {
            provider: provider.clone(),
            message: message.to_owned(),
        })
    } else {
        ProviderError::Mapping(MappingError::ContractViolation {
            provider: provider.clone(),
            message: message.to_owned(),
        })
    }
}
