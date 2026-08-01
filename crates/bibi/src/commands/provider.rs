use anyhow::Result;
use bibi_application::domain::ProviderName;
use bibi_provider::Providers;

pub fn installed(value: &str) -> Result<ProviderName> {
    Providers::installed(value).map_err(Into::into)
}
