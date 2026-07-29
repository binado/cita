//! Constructing the concrete services a command runs against.

use anyhow::{Context, Result};
use bibi_application::domain::ManifestStore;
use bibi_application::{PlatformPaths, Services, TargetResolver, TargetSelection};
use bibi_provider::{LocalProvider, Provider, ProviderRegistry};
use std::sync::Arc;

/// Build the provider roster.
///
/// Order is roster order, and roster order is resolution order. The local
/// provider comes last and declares no locator kinds, so it is never offered a
/// locator: storing a user's own entry is a decision the application makes
/// after every other provider has reported absence, not a fallback resolver.
///
/// No provider is selected by a `match` inside command code; adding one is a
/// new crate plus one line here.
pub fn providers() -> Result<ProviderRegistry> {
    let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(LocalProvider::new())];
    Ok(ProviderRegistry::new(providers))
}

/// Assemble the injected services.
pub fn services() -> Result<Services> {
    let paths = PlatformPaths::discover().context("locating bibi's platform directories")?;
    Ok(Services::new(Arc::new(providers()?), paths))
}

/// Resolve the manifest a command acts on.
pub fn store(services: &Services, selection: &TargetSelection) -> Result<ManifestStore> {
    let working_directory =
        std::env::current_dir().context("reading the current working directory")?;
    let resolver = TargetResolver::new(services.paths.clone(), working_directory);
    Ok(resolver.resolve(selection)?)
}

/// Build a resolver, for commands that also resolve input paths.
pub fn resolver(services: &Services) -> Result<TargetResolver> {
    let working_directory =
        std::env::current_dir().context("reading the current working directory")?;
    Ok(TargetResolver::new(
        services.paths.clone(),
        working_directory,
    ))
}
