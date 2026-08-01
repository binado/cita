//! Injected external collaborators.

use bibi_provider::Providers;
use std::sync::Arc;

/// Services used by networked commands.
#[derive(Clone, Debug)]
pub struct Services {
    /// Closed installed-provider facade.
    pub providers: Arc<Providers>,
}

impl Services {
    /// Assemble services.
    pub fn new(providers: Arc<Providers>) -> Self {
        Self { providers }
    }
}
