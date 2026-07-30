//! Turning a `--provider` value into a [`Provider`], or saying what would work.
//!
//! The provider set is closed and fixed at compile time, so there is exactly
//! one notion of "valid" here — installed — used identically whether the value
//! came from `add`, `sync`, or a `list`/`check` filter.

use anyhow::Result;
use bibi_provider::{Provider, Providers};

/// Parse a provider this build carries.
pub fn installed(value: &str, _providers: &Providers) -> Result<Provider> {
    Providers::installed(value).map_err(|_| {
        anyhow::anyhow!(
            "provider `{value}` not found. Installed providers: {}",
            Provider::ALL
                .into_iter()
                .map(Provider::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}
