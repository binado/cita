//! Extracting a source package, under hard bounds and with no escape.
//!
//! An archive is somebody else's data structure, so everything about this is
//! defensive: the compressed size, the decompressed size, and the file count
//! are all capped, every entry path is rebuilt from validated components, and
//! any entry that is not a plain file or directory is refused outright.

use crate::error::Error;
use std::{
    cmp,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

/// What an archive may not exceed.
#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    /// Bytes accepted before decompression.
    pub max_compressed_bytes: usize,
    /// Bytes accepted after decompression, across the whole archive.
    pub max_decompressed_bytes: u64,
    /// Regular files accepted.
    pub max_files: usize,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: 64 * 1024 * 1024,
            max_decompressed_bytes: 256 * 1024 * 1024,
            max_files: 10_000,
        }
    }
}

/// Extract a gzip-compressed tar into `destination`, which must be empty.
pub(crate) fn extract(
    bytes: &[u8],
    destination: &Path,
    id: &str,
    limits: ArchiveLimits,
) -> Result<(), Error> {
    if bytes.len() > limits.max_compressed_bytes {
        return Err(Error::unsafe_archive(
            id,
            "compressed source exceeds the size limit",
        ));
    }
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(Error::unsafe_archive(id, "response is not gzip-compressed"));
    }

    // The limit is enforced on the decompressed *stream*, so a zip bomb fails
    // while being read rather than after filling a disk.
    let decoder = LimitedReader::new(
        flate2::read::GzDecoder::new(bytes),
        limits.max_decompressed_bytes,
    );
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| Error::unsafe_archive(id, error))?;
    let mut files = 0usize;
    let mut extracted = 0u64;

    for entry in entries {
        let mut entry = entry.map_err(|error| Error::unsafe_archive(id, error))?;
        let kind = entry.header().entry_type();
        let path = entry
            .path()
            .map_err(|error| Error::unsafe_archive(id, error))?;
        let relative = safe_relative(&path)
            .ok_or_else(|| Error::unsafe_archive(id, "archive path escapes the destination"))?;
        if relative.as_os_str().is_empty() {
            if kind.is_dir() {
                continue;
            }
            return Err(Error::unsafe_archive(id, "archive entry has an empty path"));
        }
        let output = destination.join(&relative);

        if kind.is_dir() {
            std::fs::create_dir_all(&output).map_err(|source| Error::io(&output, source))?;
            continue;
        }
        // Symlinks, hard links, devices, fifos, and anything else are refused:
        // a link is a way to write outside the destination after the path
        // check has already passed.
        if !kind.is_file() {
            return Err(Error::unsafe_archive(
                id,
                "archive contains a link or special entry",
            ));
        }
        if files >= limits.max_files {
            return Err(Error::unsafe_archive(id, "archive has too many files"));
        }
        let declared = entry.size();
        if declared > limits.max_decompressed_bytes
            || extracted.saturating_add(declared) > limits.max_decompressed_bytes
        {
            return Err(Error::unsafe_archive(
                id,
                "extracted source exceeds the size limit",
            ));
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        }
        // `create_new` means a repeated path inside one archive cannot
        // overwrite what an earlier entry wrote.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|source| Error::io(&output, source))?;
        let mut written = 0u64;
        let mut buffer = [0u8; 8192];
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| Error::unsafe_archive(id, error))?;
            if read == 0 {
                break;
            }
            written = written.saturating_add(read as u64);
            // A header may understate an entry's real size, so the running
            // total is checked as the bytes actually arrive.
            if written > declared
                || extracted.saturating_add(written) > limits.max_decompressed_bytes
            {
                return Err(Error::unsafe_archive(
                    id,
                    "extracted source exceeds the size limit",
                ));
            }
            file.write_all(&buffer[..read])
                .map_err(|source| Error::io(&output, source))?;
        }
        file.sync_all()
            .map_err(|source| Error::io(&output, source))?;
        extracted = extracted.saturating_add(written);
        files += 1;
    }

    if files == 0 {
        return Err(Error::unsafe_archive(id, "archive contains no files"));
    }
    Ok(())
}

/// Rebuild an archive path from normal components only.
///
/// Absolute paths, drive prefixes, and `..` are refused rather than sanitized,
/// because an archive that contains them is not one bibi should be extracting.
fn safe_relative(path: &Path) -> Option<PathBuf> {
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

/// A reader that fails once it has produced more than `remaining` bytes.
struct LimitedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> LimitedReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            remaining: limit,
        }
    }
}

impl<R: Read> Read for LimitedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "decompressed source exceeds the size limit",
            ));
        }
        let take = cmp::min(buffer.len() as u64, self.remaining) as usize;
        let read = self.inner.read(&mut buffer[..take])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gzip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *contents).unwrap();
        }
        compress(&builder.into_inner().unwrap())
    }

    fn compress(tar: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(tar).unwrap();
        encoder.finish().unwrap()
    }

    fn raw_entry(path: &[u8], kind: tar::EntryType, contents: &[u8]) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_entry_type(kind);
        header.set_mode(0o644);
        header.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path);
        header.set_cksum();
        let mut tar = Vec::new();
        tar.extend_from_slice(header.as_bytes());
        tar.extend_from_slice(contents);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
        tar.extend_from_slice(&[0u8; 1024]);
        compress(&tar)
    }

    #[test]
    fn extracts_a_well_formed_archive() {
        let directory = tempfile::tempdir().unwrap();
        let archive = gzip(&[
            ("paper.tex", b"\\documentclass{article}"),
            ("fig/one.pdf", b"%PDF-"),
        ]);
        extract(
            &archive,
            directory.path(),
            "1207.7214",
            ArchiveLimits::default(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(directory.path().join("paper.tex")).unwrap(),
            "\\documentclass{article}"
        );
        assert!(directory.path().join("fig/one.pdf").is_file());
    }

    #[test]
    fn refuses_paths_that_escape_the_destination() {
        // Built by hand: a tar writer refuses to *produce* these, which is
        // itself a reminder that only a hostile archive contains them.
        for path in [
            &b"../escaped.tex"[..],
            &b"/etc/passwd"[..],
            &b"nested/../../escaped.tex"[..],
        ] {
            let directory = tempfile::tempdir().unwrap();
            let archive = raw_entry(path, tar::EntryType::Regular, b"x");
            let error = extract(
                &archive,
                directory.path(),
                "1207.7214",
                ArchiveLimits::default(),
            );
            assert!(
                error.is_err(),
                "{} was accepted",
                String::from_utf8_lossy(path)
            );
            assert!(!directory.path().join("escaped.tex").exists());
            assert!(
                !directory
                    .path()
                    .parent()
                    .unwrap()
                    .join("escaped.tex")
                    .exists()
            );
        }
    }

    #[test]
    fn refuses_links_and_special_entries() {
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Char,
            tar::EntryType::Block,
            tar::EntryType::Fifo,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let archive = raw_entry(b"evil", kind, b"");
            let error = extract(
                &archive,
                directory.path(),
                "1207.7214",
                ArchiveLimits::default(),
            );
            assert!(error.is_err(), "{kind:?} was accepted");
        }
    }

    #[test]
    fn enforces_every_documented_bound() {
        let directory = tempfile::tempdir().unwrap();
        let archive = gzip(&[("a.tex", b"hello"), ("b.tex", b"hello")]);

        let compressed = ArchiveLimits {
            max_compressed_bytes: 4,
            ..ArchiveLimits::default()
        };
        assert!(extract(&archive, directory.path(), "x", compressed).is_err());

        let decompressed = ArchiveLimits {
            max_decompressed_bytes: 4,
            ..ArchiveLimits::default()
        };
        assert!(extract(&archive, directory.path(), "x", decompressed).is_err());

        let files = ArchiveLimits {
            max_files: 1,
            ..ArchiveLimits::default()
        };
        assert!(extract(&archive, directory.path(), "x", files).is_err());
    }

    #[test]
    fn refuses_input_that_is_not_a_gzip_archive() {
        let directory = tempfile::tempdir().unwrap();
        assert!(extract(b"not gzip", directory.path(), "x", ArchiveLimits::default()).is_err());
        // An archive with no files is not a source package.
        let empty = gzip(&[]);
        assert!(extract(&empty, directory.path(), "x", ArchiveLimits::default()).is_err());
    }

    #[test]
    fn the_limited_reader_stops_at_its_budget() {
        let mut reader = LimitedReader::new(&b"0123456789"[..], 4);
        let mut buffer = [0u8; 10];
        assert_eq!(reader.read(&mut buffer).unwrap(), 4);
        assert!(reader.read(&mut buffer).is_err());
    }
}
