//! arXiv PDF and source download and local cache management for cita.
#![warn(missing_docs)]

mod endpoint;
mod error;
mod pdf;
mod source;
mod store;

pub use error::Error;
pub use store::{DocumentStore, DocumentStoreBuilder, arxiv_pdf_url};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Controls whether a document fetch may use or update the cache.
pub enum FetchPolicy {
    /// Return a valid cached artifact, otherwise download it.
    #[default]
    UseCache,
    /// Return only a valid cached artifact and never make a network request.
    CacheOnly,
    /// Download the artifact even when a valid cached copy exists.
    Force,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Selects the kind of arXiv artifact to fetch.
pub enum ArtifactKind {
    /// The rendered PDF document.
    #[default]
    Pdf,
    /// The latest gzip-compressed TeX source package, extracted into a directory.
    Source,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Describes whether an artifact was downloaded or found in the cache.
pub enum FetchOutcome {
    /// An artifact was downloaded and atomically stored at this path.
    Downloaded(std::path::PathBuf),
    /// A valid artifact was already cached at this path.
    Cached(std::path::PathBuf),
}

impl FetchOutcome {
    /// Return the absolute or caller-supplied-root-relative cached path.
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Downloaded(path) | Self::Cached(path) => path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{
        LimitedReader, MAX_COMPRESSED_SOURCE_BYTES, MAX_DECOMPRESSED_SOURCE_BYTES,
        MAX_SOURCE_ARCHIVE_FILES, SourceArchiveLimits, extract_source_archive_with_limits,
        publish_source,
    };
    use flate2::{Compression, write::GzEncoder};
    use reqwest::StatusCode;
    use std::{
        fs,
        io::{self, Read, Write},
        net::TcpListener,
        path::PathBuf,
        thread,
    };
    use tempfile::TempDir;

    fn server(status: &str, body: &[u8]) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        let body = body.to_owned();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
            String::from_utf8_lossy(&request[..length])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        (format!("http://{address}/"), handle)
    }

    fn gzip_tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len().try_into().unwrap());
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn gzip_raw_tar_entry(path: &[u8], entry_type: tar::EntryType, contents: &[u8]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.as_mut_bytes()[..path.len()].copy_from_slice(path);
        header.set_entry_type(entry_type);
        header.set_size(contents.len().try_into().unwrap());
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, contents).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[tokio::test]
    async fn downloads_modern_arxiv_pdf_and_reuses_cache() {
        let directory = tempfile::tempdir().unwrap();
        let (base_url, handle) = server("200 OK", b"%PDF-example");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let outcome = store
            .fetch("1207.7214", FetchPolicy::UseCache)
            .await
            .unwrap();
        let expected = directory.path().join("arxiv/1207.7214.pdf");
        assert_eq!(outcome, FetchOutcome::Downloaded(expected.clone()));
        assert_eq!(fs::read(&expected).unwrap(), b"%PDF-example");
        assert!(handle.join().unwrap().contains("GET /pdf/1207.7214 "));

        assert_eq!(
            store
                .fetch("1207.7214", FetchPolicy::UseCache)
                .await
                .unwrap(),
            FetchOutcome::Cached(expected)
        );
    }

    #[tokio::test]
    async fn stores_legacy_arxiv_ids_in_nested_paths() {
        let directory = tempfile::tempdir().unwrap();
        let (base_url, handle) = server("200 OK", b"%PDF-legacy");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let outcome = store
            .fetch("hep-th/9901001", FetchPolicy::UseCache)
            .await
            .unwrap();
        assert_eq!(
            outcome.path(),
            directory.path().join("arxiv/hep-th/9901001.pdf")
        );
        assert!(handle.join().unwrap().contains("GET /pdf/hep-th/9901001 "));
    }

    #[tokio::test]
    async fn extracts_modern_source_packages_and_reuses_the_cache() {
        let directory = tempfile::tempdir().unwrap();
        let archive = gzip_tar(&[("main.tex", b"source"), ("figures/plot.dat", b"figure")]);
        let (base_url, handle) = server("200 OK", &archive);
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let expected = directory.path().join("arxiv/1207.7214/source");
        assert_eq!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                .await
                .unwrap(),
            FetchOutcome::Downloaded(expected.clone())
        );
        assert_eq!(fs::read(expected.join("main.tex")).unwrap(), b"source");
        assert_eq!(
            fs::read(expected.join("figures/plot.dat")).unwrap(),
            b"figure"
        );
        assert!(handle.join().unwrap().contains("GET /src/1207.7214 "));

        assert_eq!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                .await
                .unwrap(),
            FetchOutcome::Cached(expected)
        );
    }

    #[tokio::test]
    async fn extracts_legacy_source_packages_at_the_legacy_cache_path() {
        let directory = tempfile::tempdir().unwrap();
        let archive = gzip_tar(&[("paper.tex", b"legacy")]);
        let (base_url, handle) = server("200 OK", &archive);
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        let outcome = store
            .fetch_artifact(
                "hep-th/9901001",
                ArtifactKind::Source,
                FetchPolicy::UseCache,
            )
            .await
            .unwrap();
        assert_eq!(
            outcome.path(),
            directory.path().join("arxiv/hep-th/9901001/source")
        );
        assert_eq!(
            fs::read(outcome.path().join("paper.tex")).unwrap(),
            b"legacy"
        );
        assert!(handle.join().unwrap().contains("GET /src/hep-th/9901001 "));
    }

    #[tokio::test]
    async fn force_replaces_source_only_after_a_valid_download() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214/source");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("old.tex"), b"old").unwrap();

        let archive = gzip_tar(&[("new.tex", b"new")]);
        let (base_url, handle) = server("200 OK", &archive);
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::Force)
                .await
                .unwrap(),
            FetchOutcome::Downloaded(_)
        ));
        handle.join().unwrap();
        assert!(!destination.join("old.tex").exists());
        assert_eq!(fs::read(destination.join("new.tex")).unwrap(), b"new");

        let (base_url, handle) = server("200 OK", b"corrupt");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::Force)
                .await,
            Err(Error::InvalidSourceArchive { .. })
        ));
        handle.join().unwrap();
        assert_eq!(fs::read(destination.join("new.tex")).unwrap(), b"new");

        let (base_url, handle) = server("404 Not Found", b"missing");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::Force)
                .await,
            Err(Error::SourceUnavailable(_))
        ));
        handle.join().unwrap();
        assert_eq!(fs::read(destination.join("new.tex")).unwrap(), b"new");
    }

    #[tokio::test]
    async fn source_cache_only_handles_hits_misses_and_invalid_directories() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214/source");
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("main.tex"), b"cached").unwrap();
        let store = DocumentStore::new(directory.path()).unwrap();

        assert_eq!(
            store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::CacheOnly)
                .await
                .unwrap(),
            FetchOutcome::Cached(destination.clone())
        );
        let missing = directory.path().join("arxiv/2101.00001/source");
        assert!(matches!(
            store
                .fetch_artifact(
                    "2101.00001",
                    ArtifactKind::Source,
                    FetchPolicy::CacheOnly
                )
                .await,
            Err(Error::SourceNotCached(path)) if path == missing
        ));
        fs::remove_file(destination.join("main.tex")).unwrap();
        assert!(matches!(
            store
                .fetch_artifact(
                    "1207.7214",
                    ArtifactKind::Source,
                    FetchPolicy::CacheOnly
                )
                .await,
            Err(Error::InvalidCachedSource(path)) if path == destination
        ));
    }

    #[tokio::test]
    async fn rejects_unavailable_and_malformed_source_responses() {
        type ErrorPredicate = fn(&Error) -> bool;
        let cases: Vec<(&str, Vec<u8>, ErrorPredicate)> = vec![
            ("404 Not Found", b"missing".to_vec(), |error| {
                matches!(error, Error::SourceUnavailable(_))
            }),
            ("200 OK", b"%PDF-only".to_vec(), |error| {
                matches!(error, Error::SourceUnavailable(_))
            }),
            ("200 OK", b"not gzip".to_vec(), |error| {
                matches!(error, Error::InvalidSourceArchive { .. })
            }),
            (
                "200 OK",
                {
                    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
                    encoder.write_all(b"not a tar").unwrap();
                    encoder.finish().unwrap()
                },
                |error| matches!(error, Error::InvalidSourceArchive { .. }),
            ),
            ("200 OK", gzip_tar(&[]), |error| {
                matches!(error, Error::InvalidSourceArchive { .. })
            }),
            ("500 Internal Server Error", b"failed".to_vec(), |error| {
                matches!(
                    error,
                    Error::HttpStatus {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        ..
                    }
                )
            }),
        ];

        for (status, body, predicate) in cases {
            let directory = tempfile::tempdir().unwrap();
            let (base_url, handle) = server(status, &body);
            let store = DocumentStore::builder(directory.path())
                .base_url(base_url)
                .build()
                .unwrap();
            let error = store
                .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                .await
                .unwrap_err();
            assert!(predicate(&error), "{error}");
            handle.join().unwrap();
            assert!(!directory.path().join("arxiv/1207.7214/source").exists());
        }
    }

    #[tokio::test]
    async fn rejects_unsafe_source_entries_without_escaping_staging() {
        let cases = [
            gzip_raw_tar_entry(b"../escape.tex", tar::EntryType::Regular, b"escape"),
            gzip_raw_tar_entry(b"/absolute.tex", tar::EntryType::Regular, b"absolute"),
            gzip_raw_tar_entry(b"link", tar::EntryType::Symlink, b""),
            gzip_raw_tar_entry(b"hardlink", tar::EntryType::Link, b""),
            gzip_raw_tar_entry(b"device", tar::EntryType::Char, b""),
            gzip_raw_tar_entry(b"block", tar::EntryType::Block, b""),
            gzip_raw_tar_entry(b"fifo", tar::EntryType::Fifo, b""),
        ];

        for archive in cases {
            let directory = tempfile::tempdir().unwrap();
            let (base_url, handle) = server("200 OK", &archive);
            let store = DocumentStore::builder(directory.path())
                .base_url(base_url)
                .build()
                .unwrap();
            assert!(matches!(
                store
                    .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
                    .await,
                Err(Error::InvalidSourceArchive { .. })
            ));
            handle.join().unwrap();
            assert!(!directory.path().join("escape.tex").exists());
            assert!(!directory.path().join("arxiv/1207.7214/source").exists());
        }
    }

    #[test]
    fn rejects_source_archives_that_exceed_size_limits() {
        let oversized = gzip_tar(&[("main.tex", b"hello world")]);
        let directory = tempfile::tempdir().unwrap();
        let error = extract_source_archive_with_limits(
            &oversized,
            directory.path(),
            "1207.7214",
            SourceArchiveLimits {
                max_compressed_bytes: oversized.len() - 1,
                max_decompressed_bytes: MAX_DECOMPRESSED_SOURCE_BYTES,
                max_files: MAX_SOURCE_ARCHIVE_FILES,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::InvalidSourceArchive { reason, .. } if reason.contains("compressed source exceeds size limit")
        ));

        let large = vec![b'a'; 8 * 1024];
        let archive = gzip_tar(&[("main.tex", large.as_slice())]);
        let directory = tempfile::tempdir().unwrap();
        let error = extract_source_archive_with_limits(
            &archive,
            directory.path(),
            "1207.7214",
            SourceArchiveLimits {
                max_compressed_bytes: MAX_COMPRESSED_SOURCE_BYTES,
                max_decompressed_bytes: 1024,
                max_files: MAX_SOURCE_ARCHIVE_FILES,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::InvalidSourceArchive { reason, .. } if reason.contains("extracted source exceeds size limit")
        ));
        assert!(directory.path().read_dir().unwrap().next().is_none());

        let archive = gzip_tar(&[("a.tex", b"a"), ("b.tex", b"b"), ("c.tex", b"c")]);
        let directory = tempfile::tempdir().unwrap();
        let error = extract_source_archive_with_limits(
            &archive,
            directory.path(),
            "1207.7214",
            SourceArchiveLimits {
                max_compressed_bytes: MAX_COMPRESSED_SOURCE_BYTES,
                max_decompressed_bytes: MAX_DECOMPRESSED_SOURCE_BYTES,
                max_files: 2,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            Error::InvalidSourceArchive { reason, .. } if reason.contains("too many files")
        ));
    }

    #[test]
    fn limited_reader_enforces_decompressed_budget() {
        let data = vec![1_u8; 32];
        let mut reader = LimitedReader::new(data.as_slice(), 8);
        let mut buffer = [0_u8; 16];
        assert_eq!(reader.read(&mut buffer).unwrap(), 8);
        assert_eq!(&buffer[..8], &[1_u8; 8]);
        let error = reader.read(&mut buffer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn rejects_oversized_compressed_source_downloads() {
        let directory = tempfile::tempdir().unwrap();
        let body = vec![0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff]
            .into_iter()
            .chain(std::iter::repeat_n(0_u8, 64))
            .collect::<Vec<_>>();
        let (base_url, handle) = server("200 OK", &body);
        let mut store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        store.source_limits = SourceArchiveLimits {
            max_compressed_bytes: 32,
            max_decompressed_bytes: MAX_DECOMPRESSED_SOURCE_BYTES,
            max_files: MAX_SOURCE_ARCHIVE_FILES,
        };
        let error = store
            .fetch_artifact("1207.7214", ArtifactKind::Source, FetchPolicy::UseCache)
            .await
            .unwrap_err();
        handle.join().unwrap();
        assert!(matches!(
            error,
            Error::InvalidSourceArchive { reason, .. } if reason.contains("compressed source exceeds size limit")
        ));
        assert!(!directory.path().join("arxiv/1207.7214/source").exists());
    }

    #[tokio::test]
    async fn force_atomically_replaces_cached_pdf() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"%PDF-old").unwrap();
        let (base_url, handle) = server("200 OK", b"%PDF-new");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();

        assert!(matches!(
            store.fetch("1207.7214", FetchPolicy::Force).await.unwrap(),
            FetchOutcome::Downloaded(_)
        ));
        assert_eq!(fs::read(destination).unwrap(), b"%PDF-new");
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn cache_only_reuses_a_valid_pdf_and_rejects_a_cache_miss() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"%PDF-cached").unwrap();
        let store = DocumentStore::new(directory.path()).unwrap();

        assert_eq!(
            store
                .fetch("1207.7214", FetchPolicy::CacheOnly)
                .await
                .unwrap(),
            FetchOutcome::Cached(destination)
        );

        let missing = directory.path().join("arxiv/2101.00001.pdf");
        assert!(matches!(
            store
                .fetch("2101.00001", FetchPolicy::CacheOnly)
                .await,
            Err(Error::NotCached(path)) if path == missing
        ));
    }

    #[test]
    fn builds_public_arxiv_pdf_urls_for_browser_opening() {
        assert_eq!(
            arxiv_pdf_url("1207.7214").unwrap().as_str(),
            "https://arxiv.org/pdf/1207.7214"
        );
        assert_eq!(
            arxiv_pdf_url("hep-th/9901001").unwrap().as_str(),
            "https://arxiv.org/pdf/hep-th/9901001"
        );
        assert!(matches!(
            arxiv_pdf_url(""),
            Err(Error::InvalidArxivIdentifier(_))
        ));
    }

    #[tokio::test]
    async fn rejects_missing_invalid_and_non_pdf_documents() {
        let directory = tempfile::tempdir().unwrap();
        let store = DocumentStore::new(directory.path()).unwrap();
        assert!(matches!(
            store.fetch("", FetchPolicy::UseCache).await,
            Err(Error::InvalidArxivIdentifier(_))
        ));
        assert!(matches!(
            store.fetch("not-an-id", FetchPolicy::UseCache).await,
            Err(Error::InvalidArxivIdentifier(_))
        ));

        let (base_url, handle) = server("200 OK", b"not a PDF");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"%PDF-existing").unwrap();
        assert!(matches!(
            store.fetch("1207.7214", FetchPolicy::Force).await,
            Err(Error::InvalidDownloadedPdf(_))
        ));
        assert_eq!(fs::read(destination).unwrap(), b"%PDF-existing");
        handle.join().unwrap();
    }

    #[tokio::test]
    async fn reports_http_and_invalid_cached_file_errors() {
        let directory = tempfile::tempdir().unwrap();
        let (base_url, handle) = server("404 Not Found", b"missing");
        let store = DocumentStore::builder(directory.path())
            .base_url(base_url)
            .build()
            .unwrap();
        assert!(matches!(
            store.fetch("1207.7214", FetchPolicy::UseCache).await,
            Err(Error::HttpStatus {
                status: StatusCode::NOT_FOUND,
                ..
            })
        ));
        handle.join().unwrap();

        let destination = directory.path().join("arxiv/1207.7214.pdf");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, b"broken").unwrap();
        assert!(matches!(
            store
                .fetch("1207.7214", FetchPolicy::UseCache)
                .await,
            Err(Error::InvalidCachedPdf(path)) if path == destination
        ));
    }

    #[test]
    fn cache_errors_are_policy_neutral() {
        let path = PathBuf::from("cache/paper.pdf");

        assert_eq!(
            Error::InvalidCachedPdf(path.clone()).to_string(),
            "cached file cache/paper.pdf is not a valid PDF"
        );
        assert_eq!(
            Error::NotCached(path).to_string(),
            "PDF is not cached at cache/paper.pdf"
        );
    }

    #[test]
    fn failed_source_publication_restores_the_previous_cache() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("source");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("old.tex"), b"old").unwrap();
        let staging = TempDir::new_in(directory.path()).unwrap();
        fs::write(staging.path().join("new.tex"), b"new").unwrap();
        fs::remove_dir_all(staging.path()).unwrap();

        assert!(matches!(
            publish_source(staging, &destination),
            Err(Error::PublishSource { .. })
        ));
        assert_eq!(fs::read(destination.join("old.tex")).unwrap(), b"old");
    }

    #[test]
    fn publish_source_installs_and_replaces_through_a_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("source");

        let staging = tempfile::Builder::new()
            .prefix(".source-")
            .tempdir_in(directory.path())
            .unwrap();
        fs::write(staging.path().join("first.tex"), b"first").unwrap();
        publish_source(staging, &destination).unwrap();
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(destination.join("first.tex")).unwrap(), b"first");
        let first_target = fs::read_link(&destination).unwrap();

        let staging = tempfile::Builder::new()
            .prefix(".source-")
            .tempdir_in(directory.path())
            .unwrap();
        fs::write(staging.path().join("second.tex"), b"second").unwrap();
        publish_source(staging, &destination).unwrap();
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(destination.join("second.tex")).unwrap(), b"second");
        assert!(!destination.join("first.tex").exists());
        let second_target = fs::read_link(&destination).unwrap();
        assert_ne!(first_target, second_target);
        assert!(!directory.path().join(&first_target).exists());
    }

    #[test]
    fn publish_source_upgrades_a_legacy_directory_cache() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("source");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("old.tex"), b"old").unwrap();

        let staging = tempfile::Builder::new()
            .prefix(".source-")
            .tempdir_in(directory.path())
            .unwrap();
        fs::write(staging.path().join("new.tex"), b"new").unwrap();
        publish_source(staging, &destination).unwrap();

        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(destination.join("new.tex")).unwrap(), b"new");
        assert!(!destination.join("old.tex").exists());
    }

    #[test]
    fn failed_symlink_replace_keeps_the_previous_published_cache() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("source");
        let staging = tempfile::Builder::new()
            .prefix(".source-")
            .tempdir_in(directory.path())
            .unwrap();
        fs::write(staging.path().join("old.tex"), b"old").unwrap();
        publish_source(staging, &destination).unwrap();

        let staging = tempfile::Builder::new()
            .prefix(".source-")
            .tempdir_in(directory.path())
            .unwrap();
        fs::write(staging.path().join("new.tex"), b"new").unwrap();
        fs::remove_dir_all(staging.path()).unwrap();

        assert!(matches!(
            publish_source(staging, &destination),
            Err(Error::PublishSource { .. })
        ));
        assert_eq!(fs::read(destination.join("old.tex")).unwrap(), b"old");
    }
}
