//! arXiv identifier validation and URL construction.

use cita_core::Locator;
use url::Url;

use crate::Error;

/// Default arXiv endpoint used when no override is configured.
pub(crate) const DEFAULT_BASE_URL: &str = "https://arxiv.org/";

pub(crate) fn validated_arxiv_id(value: &str) -> Result<String, Error> {
    match format!("arxiv:{value}").parse::<Locator>() {
        Ok(Locator::Arxiv(id)) => Ok(id),
        _ => Err(Error::InvalidArxivIdentifier(value.to_owned())),
    }
}

pub(crate) fn pdf_url(base_url: &Url, arxiv_id: &str) -> Result<Url, Error> {
    artifact_url(base_url, "pdf", arxiv_id)
}

pub(crate) fn source_url(base_url: &Url, arxiv_id: &str) -> Result<Url, Error> {
    artifact_url(base_url, "src", arxiv_id)
}

fn artifact_url(base_url: &Url, endpoint: &str, arxiv_id: &str) -> Result<Url, Error> {
    let mut url = base_url.clone();
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| Error::InvalidBaseUrl(base_url.to_string()))?;
    segments.pop_if_empty();
    segments.push(endpoint);
    for segment in arxiv_id.split('/') {
        segments.push(segment);
    }
    drop(segments);
    Ok(url)
}
