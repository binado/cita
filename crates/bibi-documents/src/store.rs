//! Downloading an arXiv artifact to a destination the caller chose.

use crate::{error::Error, naming::ArtifactKind};
use bibi_core::ArxivId;
use std::{io::Write, path::Path, time::Duration};
use url::Url;

const DEFAULT_BASE_URL: &str = "https://arxiv.org/";
const DEFAULT_USER_AGENT: &str = concat!("bibi-documents/", env!("CARGO_PKG_VERSION"));
const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;
const PDF_SIGNATURE: &[u8] = b"%PDF-";
const GZIP_SIGNATURE: &[u8] = &[0x1f, 0x8b];

/// Fetches arXiv artifacts. It owns no storage and remembers nothing.
#[derive(Clone, Debug)]
pub struct ArtifactClient {
    base_url: Url,
    http: reqwest::Client,
    max_bytes: usize,
}

impl ArtifactClient {
    /// A client against arXiv's public address.
    pub fn new() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// Begin configuring a client.
    pub fn builder() -> ArtifactClientBuilder {
        ArtifactClientBuilder {
            base_url: DEFAULT_BASE_URL.to_owned(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            timeout: Duration::from_secs(60),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Download one artifact to `destination`, which must not already exist.
    ///
    /// The bytes are validated for their kind before anything is published, and
    /// published through a temporary sibling so a destination never holds a
    /// partial download. The final rename refuses to clobber, so two concurrent
    /// fetches cannot silently produce one winner.
    pub async fn download(
        &self,
        id: &ArxivId,
        kind: ArtifactKind,
        destination: &Path,
    ) -> Result<(), Error> {
        self.download_with_progress(id, kind, destination, |_, _| {})
            .await
    }

    /// Download one artifact to `destination`, reporting per-chunk progress.
    ///
    /// `on_chunk` is invoked with `(bytes_received, content_length_if_known)`
    /// after every body chunk arrives, so a caller can drive a progress bar
    /// without taking ownership of the response stream. The closure runs on the
    /// same task as the download, so synchronous progress reporting is enough.
    /// A no-op closure preserves the silent behaviour of [`Self::download`].
    pub async fn download_with_progress<P>(
        &self,
        id: &ArxivId,
        kind: ArtifactKind,
        destination: &Path,
        mut on_chunk: P,
    ) -> Result<(), Error>
    where
        P: FnMut(usize, Option<u64>) + Send,
    {
        if destination.exists() {
            return Err(Error::DestinationExists {
                path: destination.to_path_buf(),
            });
        }
        let url = artifact_url(&self.base_url, id, kind)?;
        let bytes = self.fetch(&url, id, kind, &mut on_chunk).await?;
        validate(&bytes, id, kind)?;
        publish(destination, &bytes)
    }

    async fn fetch<P>(
        &self,
        url: &Url,
        id: &ArxivId,
        kind: ArtifactKind,
        on_chunk: &mut P,
    ) -> Result<Vec<u8>, Error>
    where
        P: FnMut(usize, Option<u64>) + Send,
    {
        let mut response =
            self.http
                .get(url.clone())
                .send()
                .await
                .map_err(|source| Error::Download {
                    url: url.to_string(),
                    source,
                })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound {
                id: id.to_string(),
                kind: kind.label(),
            });
        }
        if !status.is_success() {
            return Err(Error::Status {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }
        let too_large = || Error::ArtifactTooLarge {
            id: id.to_string(),
            kind: kind.label(),
            limit: self.max_bytes,
        };
        let total = response.content_length();
        if total.is_some_and(|length| length > self.max_bytes as u64) {
            return Err(too_large());
        }
        on_chunk(0, total);

        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|source| Error::Download {
            url: url.to_string(),
            source,
        })? {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > self.max_bytes)
            {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
            on_chunk(bytes.len(), total);
        }
        Ok(bytes)
    }
}

/// Confirm the response is the kind of file it was asked for.
///
/// arXiv answers with an HTML holding page for a withdrawn or not-yet-processed
/// work, and that is neither a PDF nor an archive. Checking the magic bytes is
/// what keeps such a page from being saved under a `.pdf` name.
fn validate(bytes: &[u8], id: &ArxivId, kind: ArtifactKind) -> Result<(), Error> {
    let (signature, reason) = match kind {
        ArtifactKind::Pdf => (PDF_SIGNATURE, "the response is not a PDF"),
        ArtifactKind::Source => (GZIP_SIGNATURE, "the response is not a gzip archive"),
    };
    if bytes.starts_with(signature) {
        Ok(())
    } else {
        Err(Error::InvalidArtifact {
            id: id.to_string(),
            reason: reason.into(),
        })
    }
}

/// Write beside the destination, sync, then rename into place without clobbering.
fn publish(destination: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = match destination.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
        Some(parent) => parent,
        None => {
            return Err(Error::io(
                destination,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "a destination must have a parent directory",
                ),
            ));
        }
    };
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| Error::io(parent, source))?;
    temporary
        .write_all(bytes)
        .map_err(|source| Error::io(destination, source))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|source| Error::io(destination, source))?;
    // `persist_noclobber` removes the temporary on failure, so a lost race
    // leaves the winner's file intact and no debris beside it.
    temporary.persist_noclobber(destination).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            Error::DestinationExists {
                path: destination.to_path_buf(),
            }
        } else {
            Error::io(destination, error.error)
        }
    })?;
    Ok(())
}

/// The public URL of a work's artifact, for opening or reporting.
///
/// A free function on arXiv's published address, because reporting a URL needs
/// no client at all: `fetch --url` must answer without building one.
pub fn public_url(id: &ArxivId, kind: ArtifactKind) -> Result<Url, Error> {
    let base = Url::parse(DEFAULT_BASE_URL).expect("the default base URL is valid");
    artifact_url(&base, id, kind)
}

/// `<base>/<endpoint>/<id>`, with the identifier's own components appended.
fn artifact_url(base_url: &Url, id: &ArxivId, kind: ArtifactKind) -> Result<Url, Error> {
    let mut url = base_url.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| Error::InvalidBaseUrl(base_url.to_string()))?;
        segments.pop_if_empty();
        segments.push(kind.endpoint());
        for segment in id.as_str().split('/') {
            segments.push(segment);
        }
    }
    Ok(url)
}

/// Configures an [`ArtifactClient`].
#[derive(Clone, Debug)]
pub struct ArtifactClientBuilder {
    base_url: String,
    user_agent: String,
    timeout: Duration,
    max_bytes: usize,
}

impl ArtifactClientBuilder {
    /// Point the client at another arXiv-compatible base URL.
    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }

    /// Override the user agent.
    pub fn user_agent(mut self, value: impl Into<String>) -> Self {
        self.user_agent = value.into();
        self
    }

    /// Override the request timeout.
    pub fn timeout(mut self, value: Duration) -> Self {
        self.timeout = value;
        self
    }

    /// Override the largest response accepted.
    pub fn max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// Validate the configuration and build the client.
    pub fn build(self) -> Result<ArtifactClient, Error> {
        let mut base_url =
            Url::parse(&self.base_url).map_err(|_| Error::InvalidBaseUrl(self.base_url.clone()))?;
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let http = reqwest::Client::builder()
            .user_agent(self.user_agent)
            .timeout(self.timeout)
            .build()
            .map_err(Error::Client)?;
        Ok(ArtifactClient {
            base_url,
            http,
            max_bytes: self.max_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_public_urls_a_browser_can_open() {
        let id = ArxivId::new("1207.7214v2").unwrap();
        assert_eq!(
            public_url(&id, ArtifactKind::Pdf).unwrap().as_str(),
            "https://arxiv.org/pdf/1207.7214"
        );
        assert_eq!(
            public_url(&id, ArtifactKind::Source).unwrap().as_str(),
            "https://arxiv.org/e-print/1207.7214"
        );
        // A legacy identifier keeps its archive as a path component.
        assert_eq!(
            public_url(&ArxivId::new("hep-th/9901001").unwrap(), ArtifactKind::Pdf)
                .unwrap()
                .as_str(),
            "https://arxiv.org/pdf/hep-th/9901001"
        );
    }

    #[test]
    fn a_holding_page_is_not_an_artifact() {
        let id = ArxivId::new("1207.7214").unwrap();
        let html = b"<!DOCTYPE html><html>";
        assert!(validate(html, &id, ArtifactKind::Pdf).is_err());
        assert!(validate(html, &id, ArtifactKind::Source).is_err());
        assert!(validate(b"%PDF-1.7\n", &id, ArtifactKind::Pdf).is_ok());
        assert!(validate(&[0x1f, 0x8b, 0x08], &id, ArtifactKind::Source).is_ok());
        // Each kind rejects the other's bytes.
        assert!(validate(b"%PDF-1.7\n", &id, ArtifactKind::Source).is_err());
        assert!(validate(&[0x1f, 0x8b, 0x08], &id, ArtifactKind::Pdf).is_err());
    }
}
