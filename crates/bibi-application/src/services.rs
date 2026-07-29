//! What a use case is given, rather than what it constructs.

use crate::{error::Error, target::PlatformPaths};
use bibi_documents::DocumentStore;
use bibi_provider::ProviderRegistry;
use std::sync::{Arc, OnceLock};

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
    /// The document cache, built on first use.
    documents: Arc<OnceLock<DocumentStore>>,
}

impl Services {
    /// Assemble the services.
    pub fn new(providers: Arc<ProviderRegistry>, paths: PlatformPaths) -> Self {
        Self {
            providers,
            paths,
            documents: Arc::new(OnceLock::new()),
        }
    }

    /// Supply a pre-built document store, as tests pointing at a local server do.
    pub fn with_documents(mut self, documents: DocumentStore) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(documents);
        self.documents = Arc::new(cell);
        self
    }

    /// The document cache.
    ///
    /// Built on demand rather than at startup, so that every offline command
    /// stays offline by construction: no HTTP client exists unless something
    /// actually needs to download a document.
    pub fn documents(&self) -> Result<&DocumentStore, Error> {
        if let Some(documents) = self.documents.get() {
            return Ok(documents);
        }
        let store = DocumentStore::new(self.paths.cache_root())?;
        let _ = self.documents.set(store);
        Ok(self.documents.get().expect("the store was just set"))
    }
}
