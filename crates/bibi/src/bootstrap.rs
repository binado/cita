//! Constructing the concrete services a command runs against.

use anyhow::{Context, Result};
use bibi_application::domain::ManifestStore;
use bibi_application::{PlatformPaths, Services, TargetResolver};
use bibi_inspire::{InspireProvider, Transport};
use bibi_provider::{LocalProvider, Provider, ProviderRegistry};
use std::{path::Path, sync::Arc};

/// Build the provider roster.
///
/// Order is roster order, and roster order is resolution order. The local
/// provider comes last and declares no locator kinds, so it is never offered a
/// locator: storing a user's own entry is a decision the application makes
/// after every other provider has reported absence, not a fallback resolver.
///
/// No provider is selected by a `match` inside command code; adding one is a
/// new crate plus one line here.
/// A test-only override for INSPIRE's base URL.
///
/// The CLI suite drives the binary against a local listener through this, so
/// that command-level tests stay hermetic. It is not a configuration surface:
/// v1 ships no configuration file, and nothing about ordinary use reads it.
pub const INSPIRE_BASE_URL_ENV: &str = "BIBI_INSPIRE_BASE_URL";

pub fn providers() -> Result<ProviderRegistry> {
    let mut builder = Transport::builder();
    if let Some(base_url) = std::env::var_os(INSPIRE_BASE_URL_ENV).filter(|value| !value.is_empty())
    {
        builder = builder.base_url(base_url.to_string_lossy().into_owned());
    }
    let transport = builder
        // Retries are reported as they happen: a command that pauses for five
        // seconds should say why rather than appear to hang.
        .on_retry(|event| {
            crate::output::warn(format!(
                "rate limited; retrying in {}s ({}/{}): {}",
                event.delay.as_secs(),
                event.attempt,
                event.max_retries,
                event.resource
            ));
        })
        .build()
        .context("building the INSPIRE client")?;
    let providers: Vec<Arc<dyn Provider>> = vec![
        Arc::new(InspireProvider::with_transport(transport)),
        Arc::new(LocalProvider::new()),
    ];
    Ok(ProviderRegistry::new(providers))
}

/// Assemble the injected services.
pub fn services() -> Result<Services> {
    let paths = PlatformPaths::discover().context("locating bibi's platform directories")?;
    Ok(Services::new(Arc::new(providers()?), paths))
}

/// Resolve the manifest a command acts on.
pub fn store(path: Option<&Path>) -> Result<ManifestStore> {
    Ok(resolver()?.resolve(path))
}

/// Build a resolver, for commands that also resolve input paths.
pub fn resolver() -> Result<TargetResolver> {
    let working_directory =
        std::env::current_dir().context("reading the current working directory")?;
    Ok(TargetResolver::new(working_directory))
}
