//! What `fetch` decides before it ever reaches the network.
//!
//! Downloading itself is covered in `bibi-documents`. What belongs here is the
//! branching: which artifact was asked for, where it would land, and whether
//! an occupied destination is a collision. None of these paths build a client,
//! which is what makes them testable without a server.

use bibi_application::{
    AddRequest, FetchOutcome, FetchRequest, FetchTarget, Progress, Services, add_locators,
    domain::ManifestStore, fetch,
};
use bibi_core::{ArxivId, Identifiers};
use bibi_documents::ArtifactClient;
use bibi_provider::{
    Provider,
    testing::{FakeProvider, provider_record, providers},
};
use std::{
    io::{BufRead, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};
use tempfile::TempDir;

struct Project {
    directory: TempDir,
    services: Services,
}

impl Project {
    fn store(&self) -> ManifestStore {
        ManifestStore::new(self.directory.path().join("bibi.toml"))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    /// A request against the project directory, as the CLI builds one.
    fn request(&self, selector: &str) -> FetchRequest {
        FetchRequest {
            selectors: vec![selector.to_owned()],
            working_directory: self.directory.path().to_path_buf(),
            ..FetchRequest::default()
        }
    }
}

/// Keep the singular safety assertions focused while the production API stays
/// batch-only. New batch behavior is tested against `fetch` directly below.
async fn fetch_one(
    services: &Services,
    store: &ManifestStore,
    request: &FetchRequest,
    progress: &mut Progress,
) -> Result<FetchTarget, String> {
    let progress = std::mem::replace(progress, Progress::silent());
    let mut progress = Some(progress);
    let report = fetch(services, store, request, |_| {
        progress.take().expect("one selector creates one reporter")
    })
    .await
    .map_err(|error| error.to_string())?;
    match report.outcomes.into_iter().next().expect("one outcome") {
        bibi_application::FetchOutcome::Success { target, .. } => Ok(target),
        bibi_application::FetchOutcome::Failure { message, .. } => Err(message),
        bibi_application::FetchOutcome::Skipped { .. } => {
            Err("unexpected duplicate in singular fetch".to_owned())
        }
    }
}

/// One INSPIRE-owned record carrying an arXiv identifier, and one local record
/// carrying none — the ordinary case of an unpublished note.
async fn project() -> Project {
    let mut managed = provider_record("inspire", "1", "Aad:2012tfa", "Observation");
    managed.identifiers = Identifiers {
        arxiv: Some(ArxivId::new("1207.7214").unwrap()),
        doi: None,
    };
    let remote = Arc::new(FakeProvider::new("inspire").with_record("arxiv:1207.7214", managed));
    let directory = tempfile::tempdir().unwrap();
    let project = Project {
        directory,
        services: Services::new(Arc::new(providers(remote))),
    };
    add_locators(
        &project.services,
        &project.store(),
        &["1207.7214".to_owned()],
        &AddRequest::default(),
    )
    .await
    .unwrap();

    let notes = project.path("notes.bib");
    std::fs::write(&notes, "@unpublished{Roe:2026,title={Lecture notes}}\n").unwrap();
    bibi_application::add_file(
        &project.services,
        &project.store(),
        &bibi_application::AddFileRequest {
            source: bibi_application::InputSource::Path(notes),
            provider: Provider::Local,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();
    project
}

async fn batch_project() -> Project {
    let directory = tempfile::tempdir().unwrap();
    let project = Project {
        directory,
        services: Services::new(Arc::new(providers(Arc::new(FakeProvider::new("inspire"))))),
    };
    let entries = project.path("batch.bib");
    std::fs::write(
        &entries,
        "@article{First,title={First},eprint={1207.7214}}\n\n\
         @article{Second,title={Second},eprint={2401.00001}}\n\n\
         @misc{Notes,title={Notes}}\n",
    )
    .unwrap();
    bibi_application::add_file(
        &project.services,
        &project.store(),
        &bibi_application::AddFileRequest {
            source: bibi_application::InputSource::Path(entries),
            provider: Provider::Local,
            overwrite: false,
            dry_run: false,
        },
    )
    .await
    .unwrap();
    project
}

#[tokio::test]
async fn a_url_is_answered_without_building_a_client() {
    let project = project().await;

    let pdf = fetch_one(
        &project.services,
        &project.store(),
        &FetchRequest {
            url: true,
            ..project.request("Aad:2012tfa")
        },
        &mut Progress::silent(),
    )
    .await
    .unwrap();
    assert_eq!(pdf.as_str(), "https://arxiv.org/pdf/1207.7214");

    // `--source --url` names the other artifact, which the previous `fetch`
    // refused to combine at all.
    let source = fetch_one(
        &project.services,
        &project.store(),
        &FetchRequest {
            url: true,
            source: true,
            ..project.request("Aad:2012tfa")
        },
        &mut Progress::silent(),
    )
    .await
    .unwrap();
    assert_eq!(source.as_str(), "https://arxiv.org/e-print/1207.7214");
}

#[tokio::test]
async fn an_already_present_default_destination_is_a_collision() {
    let project = project().await;
    let destination = project.path("1207.7214.pdf");
    std::fs::write(&destination, "mine").unwrap();

    let error = fetch_one(
        &project.services,
        &project.store(),
        &project.request("Aad:2012tfa"),
        &mut Progress::silent(),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "mine");
}

#[tokio::test]
async fn an_explicit_output_is_never_treated_as_already_present() {
    let project = project().await;
    let named = project.path("higgs.pdf");
    std::fs::write(&named, "mine").unwrap();

    // The caller named this path, so silently accepting whatever is already
    // there would be a guess about what they meant.
    let error = fetch_one(
        &project.services,
        &project.store(),
        &FetchRequest {
            output: Some(PathBuf::from("higgs.pdf")),
            ..project.request("Aad:2012tfa")
        },
        &mut Progress::silent(),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("already exists"));
    assert_eq!(std::fs::read_to_string(&named).unwrap(), "mine");
}

#[tokio::test]
async fn force_replaces_an_occupied_destination() {
    let body = b"%PDF-1.7\nnew".to_vec();
    let base_url = chunked_server(&body, 8);

    let mut project = project().await;
    project.services = project.services.with_documents(
        ArtifactClient::builder()
            .base_url(&base_url)
            .build()
            .unwrap(),
    );
    let destination = project.path("1207.7214.pdf");
    std::fs::write(&destination, "mine").unwrap();

    let target = fetch_one(
        &project.services,
        &project.store(),
        &FetchRequest {
            force: true,
            ..project.request("Aad:2012tfa")
        },
        &mut Progress::silent(),
    )
    .await
    .unwrap();

    assert!(matches!(target, FetchTarget::Downloaded(_)));
    assert_eq!(std::fs::read(&destination).unwrap(), body);
}

#[tokio::test]
async fn a_record_without_an_arxiv_identifier_says_so_rather_than_guessing() {
    let project = project().await;

    let error = fetch_one(
        &project.services,
        &project.store(),
        &FetchRequest {
            url: true,
            ..project.request("Roe:2026")
        },
        &mut Progress::silent(),
    )
    .await
    .unwrap_err();

    // No DOI resolver, no publisher page: arXiv is the only source.
    assert!(error.to_string().contains("no arXiv identifier"));
}

/// A one-shot HTTP server answering a chunked transfer-encoding reply.
///
/// Mirrors the shape used by `bibi-documents/tests/download.rs` but lives in
/// the application's integration suite, so the loop from
/// `ArtifactClient::download_with_progress` through `bibi_application::fetch`
/// into the caller's `Progress::from_fn` recorder can be exercised here.
fn chunked_server(body: &[u8], chunk_size: usize) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a test server");
    let base_url = format!("http://{}/", listener.local_addr().unwrap());
    let owned: Vec<u8> = body.to_vec();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        // Drain the request line and headers; the body is empty for a GET.
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        let _ = reader.read_line(&mut request_line);
        loop {
            let mut header = String::new();
            match reader.read_line(&mut header) {
                Ok(0) | Ok(_) if header.trim().is_empty() => break,
                Ok(_) => continue,
                Err(_) => return,
            }
        }
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        );
        for slice in owned.chunks(chunk_size) {
            let _ = write!(stream, "{:x}\r\n", slice.len());
            let _ = stream.write_all(slice);
            let _ = stream.write_all(b"\r\n");
        }
        let _ = stream.write_all(b"0\r\n\r\n");
        let _ = stream.flush();
    });
    base_url
}

#[tokio::test]
async fn a_download_progresses_through_the_recorder_closure() {
    // A PDF-signature body, served in pieces bigger than a single chunk so the
    // recorder sees at least two events.
    let body = b"%PDF-1.7\nabcdefghijklmnopqrstuvwxyz".to_vec();
    let base_url = chunked_server(&body, 8);

    let mut project = project().await;
    project.services = project.services.with_documents(
        ArtifactClient::builder()
            .base_url(&base_url)
            .build()
            .unwrap(),
    );

    let (sink, mut progress) = Progress::recorder();
    let target = fetch_one(
        &project.services,
        &project.store(),
        &project.request("Aad:2012tfa"),
        &mut progress,
    )
    .await
    .unwrap();

    let FetchTarget::Downloaded(path) = &target else {
        panic!("expected Downloaded, got {target:?}");
    };
    assert_eq!(std::fs::read(path).unwrap(), body);

    let events = sink.events();
    assert!(events.len() >= 2, "{events:?}");
    assert_eq!(events[0], (0, None));
    let last = *events.last().unwrap();
    assert_eq!(last, (body.len(), None));
    for pair in events.windows(2) {
        assert!(pair[1].0 >= pair[0].0, "{events:?}");
    }
}

#[tokio::test]
async fn a_silent_progress_passes_through_a_real_download_uneventfully() {
    let body = b"%PDF-1.7\nabcdefghijklmnopqrstuvwxyz".to_vec();
    let base_url = chunked_server(&body, 8);

    let mut project = project().await;
    project.services = project.services.with_documents(
        ArtifactClient::builder()
            .base_url(&base_url)
            .build()
            .unwrap(),
    );

    let mut progress = Progress::silent();
    let target = fetch_one(
        &project.services,
        &project.store(),
        &project.request("Aad:2012tfa"),
        &mut progress,
    )
    .await
    .unwrap();

    assert!(matches!(target, FetchTarget::Downloaded(_)));
    progress.finish();
}

#[tokio::test]
async fn a_url_batch_preserves_order_and_reports_every_selector() {
    let project = batch_project().await;
    let request = FetchRequest {
        selectors: ["First", "missing", "Second", "Notes", "1207.7214"]
            .into_iter()
            .map(ToOwned::to_owned)
            .collect(),
        url: true,
        working_directory: project.directory.path().to_path_buf(),
        ..FetchRequest::default()
    };
    let mut reporters = Vec::new();

    let report = fetch(&project.services, &project.store(), &request, |selector| {
        reporters.push(selector.to_owned());
        Progress::silent()
    })
    .await
    .unwrap();

    assert_eq!(
        reporters,
        ["First", "missing", "Second", "Notes", "1207.7214"]
    );
    assert!(matches!(
        &report.outcomes[0],
        FetchOutcome::Success {
            target: FetchTarget::Url(url),
            ..
        } if url.ends_with("/pdf/1207.7214")
    ));
    assert!(matches!(&report.outcomes[1], FetchOutcome::Failure { .. }));
    assert!(matches!(
        &report.outcomes[2],
        FetchOutcome::Success {
            target: FetchTarget::Url(url),
            ..
        } if url.ends_with("/pdf/2401.00001")
    ));
    assert!(matches!(&report.outcomes[3], FetchOutcome::Failure { .. }));
    assert!(matches!(
        &report.outcomes[4],
        FetchOutcome::Skipped { first, .. } if first == "First"
    ));
    assert!(report.has_failures());
}

/// Serve several artifacts and record their request order.
fn artifact_server(bodies: Vec<Vec<u8>>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding a test server");
    let base_url = format!("http://{}/", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    thread::spawn(move || {
        for body in bodies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                return;
            }
            recorded.lock().unwrap().push(
                request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned(),
            );
            loop {
                let mut header = String::new();
                match reader.read_line(&mut header) {
                    Ok(0) => break,
                    Ok(_) if header.trim().is_empty() => break,
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    (base_url, requests)
}

#[tokio::test]
async fn downloads_are_sequential_and_an_output_directory_uses_default_names() {
    let (base_url, requests) = artifact_server(vec![
        b"%PDF-1.7\nfirst".to_vec(),
        b"%PDF-1.7\nsecond".to_vec(),
    ]);
    let mut project = batch_project().await;
    project.services = project.services.with_documents(
        ArtifactClient::builder()
            .base_url(&base_url)
            .build()
            .unwrap(),
    );
    std::fs::create_dir(project.path("downloads")).unwrap();
    let request = FetchRequest {
        selectors: vec!["Second".to_owned(), "First".to_owned()],
        output: Some(PathBuf::from("downloads")),
        working_directory: project.directory.path().to_path_buf(),
        ..FetchRequest::default()
    };

    let report = fetch(&project.services, &project.store(), &request, |_| {
        Progress::silent()
    })
    .await
    .unwrap();

    assert!(!report.has_failures());
    assert_eq!(
        requests.lock().unwrap().as_slice(),
        ["/pdf/2401.00001", "/pdf/1207.7214"]
    );
    assert_eq!(
        std::fs::read(project.path("downloads/2401.00001.pdf")).unwrap(),
        b"%PDF-1.7\nfirst"
    );
    assert_eq!(
        std::fs::read(project.path("downloads/1207.7214.pdf")).unwrap(),
        b"%PDF-1.7\nsecond"
    );
}

#[tokio::test]
async fn multiple_selectors_require_an_existing_output_directory() {
    let project = batch_project().await;
    let request = FetchRequest {
        selectors: vec!["First".to_owned(), "Second".to_owned()],
        output: Some(PathBuf::from("missing")),
        working_directory: project.directory.path().to_path_buf(),
        ..FetchRequest::default()
    };

    let error = fetch(&project.services, &project.store(), &request, |_| {
        Progress::silent()
    })
    .await
    .unwrap_err();

    assert!(error.to_string().contains("existing directory"));
    assert!(!project.path("missing").exists());
}

#[tokio::test]
async fn occupied_destinations_fail_per_item_and_force_replaces_them() {
    let (base_url, requests) = artifact_server(vec![b"%PDF-1.7\nsecond".to_vec()]);
    let mut project = batch_project().await;
    project.services = project.services.with_documents(
        ArtifactClient::builder()
            .base_url(&base_url)
            .build()
            .unwrap(),
    );
    std::fs::write(project.path("1207.7214.pdf"), "mine").unwrap();
    let request = FetchRequest {
        selectors: vec!["First".to_owned(), "Second".to_owned()],
        working_directory: project.directory.path().to_path_buf(),
        ..FetchRequest::default()
    };

    let report = fetch(&project.services, &project.store(), &request, |_| {
        Progress::silent()
    })
    .await
    .unwrap();

    assert!(matches!(&report.outcomes[0], FetchOutcome::Failure { .. }));
    assert!(matches!(&report.outcomes[1], FetchOutcome::Success { .. }));
    assert_eq!(
        std::fs::read_to_string(project.path("1207.7214.pdf")).unwrap(),
        "mine"
    );
    assert_eq!(requests.lock().unwrap().as_slice(), ["/pdf/2401.00001"]);

    let (force_url, force_requests) =
        artifact_server(vec![b"%PDF-1.7\nfirst replacement".to_vec()]);
    project.services = project.services.with_documents(
        ArtifactClient::builder()
            .base_url(&force_url)
            .build()
            .unwrap(),
    );
    let forced = FetchRequest {
        selectors: vec!["First".to_owned()],
        force: true,
        working_directory: project.directory.path().to_path_buf(),
        ..FetchRequest::default()
    };
    let report = fetch(&project.services, &project.store(), &forced, |_| {
        Progress::silent()
    })
    .await
    .unwrap();
    assert!(!report.has_failures());
    assert_eq!(
        std::fs::read(project.path("1207.7214.pdf")).unwrap(),
        b"%PDF-1.7\nfirst replacement"
    );
    assert_eq!(force_requests.lock().unwrap().len(), 1);
}
