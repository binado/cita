//! Construct concrete services and explicit project targets.

use anyhow::{Context, Result};
use bibi_application::{Services, TargetResolver, domain::BibliographyStore};
use bibi_provider::Providers;
use std::{path::Path, sync::Arc};

pub const INSPIRE_BASE_URL_ENV: &str = "BIBI_INSPIRE_BASE_URL";

pub fn providers() -> Result<Providers> {
    let mut builder = Providers::builder();
    if let Some(value) = std::env::var_os(INSPIRE_BASE_URL_ENV).filter(|value| !value.is_empty()) {
        builder = builder.inspire_base_url(value.to_string_lossy().into_owned());
    }
    builder
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
        .context("building provider")
}

pub fn services() -> Result<Services> {
    Ok(Services::new(Arc::new(providers()?)))
}

pub fn store(path: Option<&Path>) -> Result<BibliographyStore> {
    Ok(resolver()?.resolve(path))
}

pub fn resolver() -> Result<TargetResolver> {
    Ok(TargetResolver::new(
        std::env::current_dir().context("reading working directory")?,
    ))
}
