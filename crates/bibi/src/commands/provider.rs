//! Turning a `--provider` value into a name, or saying what would have worked.
//!
//! The grammar [`ProviderName`] enforces is an internal detail. A user who
//! typed `TEST` wants to be told what to type instead, not shown a regex, and a
//! user who typed a well-formed name nothing knows wants to be told that at all
//! rather than handed an empty listing. Those are the same mistake, so they get
//! the same message.

use anyhow::{Result, bail};
use bibi_application::domain::{Manifest, ProviderName};
use bibi_provider::{Provider, Providers};

/// Parse a provider this build must actually carry.
///
/// `add` and `sync` *call* the provider they are pointed at, so a name this
/// build does not carry can never work for them, whatever the manifest says.
pub fn installed(value: &str, _providers: &Providers) -> Result<Provider> {
    Providers::installed(value).map_err(|_| {
        anyhow::anyhow!(
            "provider `{value}` not found. Installed providers: {}",
            Providers::installed_names()
                .iter()
                .map(ProviderName::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// Parse a provider a filter can meaningfully name.
///
/// Accepts a name this build carries *or* one this manifest already attributes
/// a record to. The union is what keeps a manifest written by a later build
/// usable: its `ads` records stay filterable here even though this build cannot
/// refresh them, while a name nothing knows fails instead of quietly listing
/// nothing.
pub fn selectable(value: &str, providers: &Providers, manifest: &Manifest) -> Result<ProviderName> {
    validate(value, &known(providers, manifest), "Known")
}

/// Installed names in roster order, then any the manifest alone knows, sorted.
///
/// Roster order first because that is the order resolution consults them in, so
/// it is the order that means something to a user reading the list.
fn known(_providers: &Providers, manifest: &Manifest) -> Vec<ProviderName> {
    let mut names = Providers::installed_names();
    let mut stored = manifest
        .records()
        .iter()
        .map(|record| record.provenance.provider.clone())
        .filter(|name| !names.contains(name))
        .collect::<Vec<_>>();
    stored.sort();
    stored.dedup();
    names.extend(stored);
    names
}

fn validate(value: &str, known: &[ProviderName], noun: &str) -> Result<ProviderName> {
    match ProviderName::new(value) {
        Ok(name) if known.contains(&name) => Ok(name),
        _ => bail!(
            "provider `{value}` not found. {noun} providers: {}",
            known
                .iter()
                .map(ProviderName::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
