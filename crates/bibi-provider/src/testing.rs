//! Narrow scripted construction for facade and application tests.

use crate::Providers;
use std::sync::Arc;

pub use bibi_core::provider::{
    PayloadItem, PayloadRequest, ProviderMetadata, RefreshItem, RefreshRequest, RefreshState,
    Resolution,
    testing::{
        FakeProvider, ProviderCall, payload, provider_metadata, provider_record, verify_contract,
    },
};

/// Construct a facade whose INSPIRE arm is scripted.
///
/// This is not a registration API: there remains exactly one remote dispatch
/// slot, and its fake must use the installed `inspire` provenance name.
pub fn providers(remote: Arc<FakeProvider>) -> Providers {
    assert_eq!(
        bibi_core::provider::RemoteProvider::name(remote.as_ref()).as_str(),
        "inspire",
        "the scripted facade replaces only the INSPIRE implementation"
    );
    Providers::scripted(remote)
}
