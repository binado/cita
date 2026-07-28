//! Source package extraction, validation, and atomic cache publication.

use std::{
    cmp, fmt, fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use flate2::read::GzDecoder;
use tempfile::TempDir;

use crate::Error;

/// Upper bound on the compressed `/src/` response body.
pub(crate) const MAX_COMPRESSED_SOURCE_BYTES: usize = 64 * 1024 * 1024;
/// Upper bound on gzip-decompressed bytes read while parsing the tar stream.
pub(crate) const MAX_DECOMPRESSED_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
/// Upper bound on regular files extracted from one source package.
pub(crate) const MAX_SOURCE_ARCHIVE_FILES: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceArchiveLimits {
    pub(crate) max_compressed_bytes: usize,
    pub(crate) max_decompressed_bytes: u64,
    pub(crate) max_files: usize,
}

pub(crate) const DEFAULT_SOURCE_LIMITS: SourceArchiveLimits = SourceArchiveLimits {
    max_compressed_bytes: MAX_COMPRESSED_SOURCE_BYTES,
    max_decompressed_bytes: MAX_DECOMPRESSED_SOURCE_BYTES,
    max_files: MAX_SOURCE_ARCHIVE_FILES,
};

pub(crate) fn source_cache_path(cache_root: &Path, arxiv_id: &str) -> PathBuf {
    let mut path = cache_root.join("arxiv");
    for segment in arxiv_id.split('/') {
        path.push(segment);
    }
    path.push("source");
    path
}

pub(crate) fn cache_entry_exists(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::InvalidCachedSource(path.to_owned())),
    }
}

pub(crate) fn validate_cached_source(path: &Path) -> Result<(), Error> {
    let root = fs::metadata(path).map_err(|_| Error::InvalidCachedSource(path.to_owned()))?;
    if !root.is_dir() {
        return Err(Error::InvalidCachedSource(path.to_owned()));
    }

    fn inspect(path: &Path, found_file: &mut bool) -> Result<(), std::io::Error> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.is_file() {
            *found_file = true;
            return Ok(());
        }
        if !metadata.is_dir() {
            return Err(std::io::Error::other("unsupported cache entry type"));
        }
        for entry in fs::read_dir(path)? {
            inspect(&entry?.path(), found_file)?;
        }
        Ok(())
    }

    let mut found_file = false;
    for entry in fs::read_dir(path).map_err(|_| Error::InvalidCachedSource(path.to_owned()))? {
        let entry = entry.map_err(|_| Error::InvalidCachedSource(path.to_owned()))?;
        inspect(&entry.path(), &mut found_file)
            .map_err(|_| Error::InvalidCachedSource(path.to_owned()))?;
    }
    if !found_file {
        return Err(Error::InvalidCachedSource(path.to_owned()));
    }
    Ok(())
}

pub(crate) fn extract_source_archive_with_limits(
    bytes: &[u8],
    destination: &Path,
    arxiv_id: &str,
    limits: SourceArchiveLimits,
) -> Result<(), Error> {
    if bytes.len() > limits.max_compressed_bytes {
        return Err(invalid_source_archive(
            arxiv_id,
            "compressed source exceeds size limit",
        ));
    }
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(invalid_source_archive(
            arxiv_id,
            "response is not gzip-compressed",
        ));
    }

    let decoder = LimitedReader::new(GzDecoder::new(bytes), limits.max_decompressed_bytes);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| invalid_source_archive(arxiv_id, error))?;
    let mut file_count = 0_usize;
    let mut extracted_bytes = 0_u64;

    for entry in entries {
        let mut entry = entry.map_err(|error| invalid_source_archive(arxiv_id, error))?;
        let path = entry
            .path()
            .map_err(|error| invalid_source_archive(arxiv_id, error))?;
        let entry_type = entry.header().entry_type();
        let relative = safe_relative_archive_path(&path)
            .ok_or_else(|| invalid_source_archive(arxiv_id, "archive path is unsafe"))?;
        if relative.as_os_str().is_empty() {
            if entry_type.is_dir() {
                continue;
            }
            return Err(invalid_source_archive(
                arxiv_id,
                "archive contains an empty file path",
            ));
        }
        let output = destination.join(relative);

        if entry_type.is_dir() {
            fs::create_dir_all(&output).map_err(|source| Error::ExtractSource {
                path: output,
                source,
            })?;
        } else if entry_type.is_file() {
            if file_count >= limits.max_files {
                return Err(invalid_source_archive(
                    arxiv_id,
                    "source archive has too many files",
                ));
            }
            let entry_size = entry.size();
            let next_total = extracted_bytes.saturating_add(entry_size);
            if entry_size > limits.max_decompressed_bytes
                || next_total > limits.max_decompressed_bytes
            {
                return Err(invalid_source_archive(
                    arxiv_id,
                    "extracted source exceeds size limit",
                ));
            }
            let parent = output
                .parent()
                .expect("a validated relative archive path has a parent");
            fs::create_dir_all(parent).map_err(|source| Error::ExtractSource {
                path: parent.to_owned(),
                source,
            })?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|source| Error::ExtractSource {
                    path: output.clone(),
                    source,
                })?;
            let mut written = 0_u64;
            let mut buffer = [0_u8; 8192];
            loop {
                let length = entry
                    .read(&mut buffer)
                    .map_err(|error| invalid_source_archive(arxiv_id, error))?;
                if length == 0 {
                    break;
                }
                written = written.saturating_add(length as u64);
                if written > entry_size
                    || extracted_bytes.saturating_add(written) > limits.max_decompressed_bytes
                {
                    return Err(invalid_source_archive(
                        arxiv_id,
                        "extracted source exceeds size limit",
                    ));
                }
                file.write_all(&buffer[..length])
                    .map_err(|source| Error::ExtractSource {
                        path: output.clone(),
                        source,
                    })?;
            }
            file.sync_all().map_err(|source| Error::ExtractSource {
                path: output,
                source,
            })?;
            extracted_bytes = extracted_bytes.saturating_add(written);
            file_count += 1;
        } else {
            return Err(invalid_source_archive(
                arxiv_id,
                "archive contains a link or special entry",
            ));
        }
    }

    if file_count == 0 {
        return Err(invalid_source_archive(arxiv_id, "archive is empty"));
    }
    Ok(())
}

pub(crate) struct LimitedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> LimitedReader<R> {
    pub(crate) fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "decompressed source exceeds size limit",
            ));
        }
        let max = cmp::min(buf.len() as u64, self.remaining) as usize;
        let read = self.inner.read(&mut buf[..max])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

fn safe_relative_archive_path(path: &Path) -> Option<PathBuf> {
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => relative.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(relative)
}

pub(crate) fn invalid_source_archive(arxiv_id: &str, reason: impl fmt::Display) -> Error {
    Error::InvalidSourceArchive {
        arxiv_id: arxiv_id.to_owned(),
        reason: reason.to_string(),
    }
}

pub(crate) fn publish_source(staging: TempDir, destination: &Path) -> Result<(), Error> {
    let parent = destination
        .parent()
        .expect("a source cache destination always has a parent");
    let durable = staging.keep();
    if !durable.is_dir() {
        let _ = fs::remove_dir_all(&durable);
        return Err(Error::PublishSource {
            path: destination.to_owned(),
            source: io::Error::new(
                io::ErrorKind::NotFound,
                "staging source directory is missing",
            ),
        });
    }

    let target_name = durable.file_name().ok_or_else(|| Error::PublishSource {
        path: destination.to_owned(),
        source: io::Error::new(
            io::ErrorKind::InvalidInput,
            "staging source directory has no file name",
        ),
    })?;
    let link_tmp = parent.join(format!(".link-{}", target_name.to_string_lossy()));

    let publish_result = (|| {
        symlink_dir(target_name, &link_tmp).map_err(|source| Error::PublishSource {
            path: destination.to_owned(),
            source,
        })?;

        if !cache_entry_exists(destination)? {
            fs::rename(&link_tmp, destination).map_err(|source| Error::PublishSource {
                path: destination.to_owned(),
                source,
            })?;
            return Ok(None);
        }

        let metadata =
            fs::symlink_metadata(destination).map_err(|source| Error::PublishSource {
                path: destination.to_owned(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            let previous = read_symlink_target(parent, destination)?;
            fs::rename(&link_tmp, destination).map_err(|source| Error::PublishSource {
                path: destination.to_owned(),
                source,
            })?;
            return Ok(previous);
        }

        // Legacy plain-directory caches: move aside, then install the symlink.
        let backup = parent.join(format!(".source-backup-{}", target_name.to_string_lossy()));
        fs::rename(destination, &backup).map_err(|source| Error::PublishSource {
            path: destination.to_owned(),
            source,
        })?;
        if let Err(source) = fs::rename(&link_tmp, destination) {
            if let Err(restore_error) = fs::rename(&backup, destination) {
                return Err(Error::PublishSource {
                    path: destination.to_owned(),
                    source: io::Error::other(format!(
                        "{source}; restoring the previous cache also failed: {restore_error}"
                    )),
                });
            }
            return Err(Error::PublishSource {
                path: destination.to_owned(),
                source,
            });
        }
        Ok(Some(backup))
    })();

    match publish_result {
        Ok(previous) => {
            if let Some(previous) = previous
                && previous != durable
            {
                let _ = fs::remove_dir_all(previous);
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&link_tmp);
            let _ = fs::remove_dir_all(&durable);
            Err(error)
        }
    }
}

fn read_symlink_target(parent: &Path, link: &Path) -> Result<Option<PathBuf>, Error> {
    let target = fs::read_link(link).map_err(|source| Error::PublishSource {
        path: link.to_owned(),
        source,
    })?;
    let resolved = if target.is_absolute() {
        target
    } else {
        parent.join(target)
    };
    Ok(Some(resolved))
}

fn symlink_dir(original: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(original, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(original, link)
    }
}
