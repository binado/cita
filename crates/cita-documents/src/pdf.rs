//! PDF signature validation and cache path layout.

use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use crate::Error;

/// Leading bytes that identify a PDF document.
pub(crate) const PDF_SIGNATURE: &[u8] = b"%PDF-";

pub(crate) fn pdf_cache_path(cache_root: &Path, arxiv_id: &str) -> PathBuf {
    let mut path = cache_root.join("arxiv");
    if let Some((archive, number)) = arxiv_id.split_once('/') {
        path.push(archive);
        path.push(format!("{number}.pdf"));
    } else {
        path.push(format!("{arxiv_id}.pdf"));
    }
    path
}

pub(crate) fn validate_cached_pdf(path: &Path) -> Result<(), Error> {
    let mut file = fs::File::open(path).map_err(|source| Error::InspectCachedPdf {
        path: path.to_owned(),
        source,
    })?;
    if has_pdf_signature(&mut file).map_err(|source| Error::InspectCachedPdf {
        path: path.to_owned(),
        source,
    })? {
        Ok(())
    } else {
        Err(Error::InvalidCachedPdf(path.to_owned()))
    }
}

pub(crate) fn has_pdf_signature(file: &mut fs::File) -> Result<bool, std::io::Error> {
    file.seek(SeekFrom::Start(0))?;
    let mut signature = [0_u8; PDF_SIGNATURE.len()];
    let length = file.read(&mut signature)?;
    Ok(length == PDF_SIGNATURE.len() && signature == PDF_SIGNATURE)
}
