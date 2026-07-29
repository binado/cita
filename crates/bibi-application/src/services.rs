//! What a use case is given, rather than what it constructs.

use crate::target::PlatformPaths;
use bibi_provider::ProviderRegistry;
use std::sync::Arc;

/// The collaborators every use case shares.
///
/// Injected rather than constructed, so tests supply fake providers and
/// temporary directories, and so no use case decides which providers exist.
#[derive(Clone, Debug)]
pub struct Services {
    /// The installed providers, in roster order.
    pub providers: Arc<ProviderRegistry>,
    /// The platform's locations.
    pub paths: PlatformPaths,
}

impl Services {
    /// Assemble the services.
    pub fn new(providers: Arc<ProviderRegistry>, paths: PlatformPaths) -> Self {
        Self { providers, paths }
    }
}
