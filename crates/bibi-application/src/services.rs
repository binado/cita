//! What a use case is given, rather than what it constructs.

use crate::error::Error;
use bibi_documents::ArtifactClient;
use bibi_provider::ProviderRegistry;
use std::sync::{Arc, OnceLock};

/// The collaborators every use case shares.
///
/// Injected rather than constructed, so tests supply fake providers and a
/// client pointed at a local listener, and so no use case decides which
/// providers exist.
#[derive(Clone, Debug)]
pub struct Services {
    /// The installed providers, in roster order.
    pub providers: Arc<ProviderRegistry>,
    /// The arXiv artifact client, built on first use.
    documents: Arc<OnceLock<ArtifactClient>>,
}

impl Services {
    /// Assemble the services.
    pub fn new(providers: Arc<ProviderRegistry>) -> Self {
        Self {
            providers,
            documents: Arc::new(OnceLock::new()),
        }
    }

    /// Supply a pre-built client, as tests pointing at a local server do.
    pub fn with_documents(mut self, documents: ArtifactClient) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(documents);
        self.documents = Arc::new(cell);
        self
    }

    /// The arXiv artifact client.
    ///
    /// Built on demand rather than at startup, so that every offline command
    /// stays offline by construction: no HTTP client exists unless something
    /// actually needs to download a document.
    pub fn documents(&self) -> Result<&ArtifactClient, Error> {
        if let Some(documents) = self.documents.get() {
            return Ok(documents);
        }
        let client = ArtifactClient::new()?;
        let _ = self.documents.set(client);
        Ok(self.documents.get().expect("the client was just set"))
    }
}
