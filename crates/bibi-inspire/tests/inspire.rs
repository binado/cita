//! The INSPIRE provider against a local server: queries, joins, and pacing.

mod support;

use bibi_core::{ArxivId, BibiId, Doi, Locator, ProviderId};
use bibi_inspire::{InspireProvider, RateLimiter, RetryEvent, Transport, testing::TestClock};
use bibi_provider::{
    Provider, ProviderError, RefreshRequest, RefreshState, Resolution, RetrievalError,
    testing::verify_contract,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use support::{Reply, TestServer};

/// A JSON search response holding one record.
fn record(id: u64, texkey: &str, arxiv: Option<&str>, doi: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "updated": format!("2026-01-0{}T00:00:00+00:00", id % 9 + 1),
        "metadata": {
            "control_number": id,
            "texkeys": [texkey],
            "titles": [{"title": format!("Record {id}")}],
            "authors": [{"full_name": "Aad, G."}],
            "collaborations": [{"value": "ATLAS"}],
            "publication_info": [{"year": 2012}],
            "arxiv_eprints": arxiv.map(|value| vec![serde_json::json!({"value": value})]).unwrap_or_default(),
            "dois": doi.map(|value| vec![serde_json::json!({"value": value})]).unwrap_or_default(),
        }
    })
}

fn hits(records: Vec<serde_json::Value>) -> String {
    serde_json::json!({"hits": {"hits": records}}).to_string()
}

fn entry(texkey: &str) -> String {
    format!("@article{{{texkey},\n  title = {{Provider formatting}}\n}}\n")
}

/// A provider pointed at a test server, with a clock that never really sleeps.
fn provider(server: &TestServer, clock: Arc<TestClock>) -> InspireProvider {
    InspireProvider::with_transport(
        Transport::builder()
            .base_url(&server.base_url)
            .clock(clock)
            .build()
            .unwrap(),
    )
}

#[tokio::test]
async fn one_batched_search_resolves_mixed_locator_kinds() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![
            record(1, "First:2012", Some("1207.7214"), None),
            record(2, "Second:2013", None, Some("10.1/b")),
            record(3, "Third:2014", None, None),
        ])),
        Reply::ok(format!(
            "{}\n{}\n{}",
            entry("First:2012"),
            entry("Second:2013"),
            entry("Third:2014")
        )),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let locators = [
        Locator::Arxiv(ArxivId::new("1207.7214").unwrap()),
        Locator::Doi(Doi::new("10.1/b").unwrap()),
        Locator::ProviderId("3".into()),
    ];

    let resolutions = provider.resolve(&locators).await.unwrap();
    assert_eq!(resolutions.len(), 3, "one answer per locator, positionally");
    for resolution in &resolutions {
        assert!(matches!(resolution, Resolution::Found(_)), "{resolution:?}");
    }
    let Resolution::Found(first) = &resolutions[0] else {
        unreachable!()
    };
    assert_eq!(first.provenance.provider.as_str(), "inspire");
    assert_eq!(first.provenance.provider_id.as_ref().unwrap().as_str(), "1");
    assert_eq!(first.description.collaborations, ["ATLAS"]);
    assert_eq!(first.payload.source_key().as_str(), "First:2012");

    // Three locators, two requests: one JSON search and one BibTeX search.
    assert_eq!(server.requests().len(), 2);
    let query = server.query(0);
    assert!(query.contains("arxiv:1207.7214"), "{query}");
    assert!(query.contains("doi:10.1/b"), "{query}");
    assert!(query.contains("control_number:3"), "{query}");
    assert!(query.contains(" or "), "one query, not three");
    // The metadata request asks only for the fields the record model uses.
    assert!(
        query.contains("fields=control_number,texkeys,titles"),
        "{query}"
    );
    assert!(server.query(1).contains("format=bibtex"));
}

#[tokio::test]
async fn a_locator_no_record_answers_is_not_found_while_its_neighbours_resolve() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![record(1, "First:2012", Some("1207.7214"), None)])),
        Reply::ok(entry("First:2012")),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let resolutions = provider
        .resolve(&[
            Locator::Arxiv(ArxivId::new("1207.7214").unwrap()),
            Locator::Arxiv(ArxivId::new("2401.00001").unwrap()),
        ])
        .await
        .unwrap();
    assert!(matches!(resolutions[0], Resolution::Found(_)));
    assert!(matches!(resolutions[1], Resolution::NotFound));
}

#[tokio::test]
async fn two_locators_for_one_paper_both_resolve_to_it() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![record(
            1,
            "First:2012",
            Some("1207.7214"),
            Some("10.1/a"),
        )])),
        Reply::ok(entry("First:2012")),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let resolutions = provider
        .resolve(&[
            Locator::Arxiv(ArxivId::new("1207.7214").unwrap()),
            Locator::Doi(Doi::new("10.1/a").unwrap()),
        ])
        .await
        .unwrap();

    // The provider does not deduplicate on the caller's behalf; both answers
    // are the same record, and the application's duplicate policy collapses them.
    let (Resolution::Found(left), Resolution::Found(right)) = (&resolutions[0], &resolutions[1])
    else {
        panic!("both locators should resolve");
    };
    assert_eq!(left.provenance.provider_id, right.provenance.provider_id);
    // One payload fetch, not two.
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn a_locator_kind_inspire_cannot_read_is_unsupported_not_absent() {
    let server = TestServer::new(vec![]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let resolutions = provider
        .resolve(&[Locator::ProviderId("2024ApJ...900..1X".into())])
        .await
        .unwrap();
    assert!(matches!(resolutions[0], Resolution::UnsupportedLocator));
    // Unsupported costs no request at all.
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn an_http_failure_is_a_retrieval_error_rather_than_absence() {
    let server = TestServer::new(vec![Reply::status(500, "upstream is unwell")]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let error = provider
        .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ProviderError::Retrieval(RetrievalError::Status { status: 500, .. })
    ));
}

#[tokio::test]
async fn an_entry_matching_no_requested_record_fails_the_whole_batch() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![record(1, "First:2012", Some("1207.7214"), None)])),
        Reply::ok(entry("Stranger:1999")),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let error = provider
        .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Mapping(_)));
    assert!(error.to_string().contains("Stranger:1999"));
}

#[tokio::test]
async fn a_malformed_or_multiple_entry_payload_is_refused() {
    for body in ["@article{Broken,title={", "not bibtex at all"] {
        let server = TestServer::new(vec![
            Reply::ok(hits(vec![record(1, "First:2012", Some("1207.7214"), None)])),
            Reply::ok(body),
        ]);
        let provider = provider(&server, Arc::new(TestClock::new()));
        assert!(
            provider
                .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
                .await
                .is_err(),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_record_resolved_but_never_rendered_is_refused_rather_than_stored() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![record(1, "First:2012", Some("1207.7214"), None)])),
        Reply::ok(""),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let error = provider
        .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no BibTeX entry"));
}

#[tokio::test]
async fn refresh_asks_only_for_the_narrowed_fields_and_answers_every_request() {
    let server = TestServer::new(vec![Reply::ok(hits(vec![
        record(1, "First:2012", None, None),
        record(2, "Second:2013", None, None),
    ]))]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let requests = [1u64, 2, 3]
        .map(|id| RefreshRequest {
            bibi_id: BibiId::new(),
            provider_id: ProviderId::new(id.to_string()).unwrap(),
            stored_revision: None,
        })
        .to_vec();

    let items = provider.refresh_metadata(&requests).await;
    assert_eq!(items.len(), 3, "one outcome per requested record");
    assert!(matches!(
        items[0].result,
        Ok(RefreshState::Metadata(ref metadata)) if metadata.revision.is_some()
    ));
    // A record INSPIRE no longer holds is absence, not a failure.
    assert!(matches!(items[2].result, Ok(RefreshState::Missing)));

    let query = server.query(0);
    assert!(query.contains("control_number:1 or control_number:2 or control_number:3"));
    assert!(query.contains("fields="), "{query}");
    assert!(!query.contains("format=bibtex"), "metadata only");
}

#[tokio::test]
async fn a_payload_fetch_reuses_the_texkeys_the_metadata_pass_learned() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![record(1, "First:2012", None, None)])),
        Reply::ok(entry("First:2012")),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let requests = vec![RefreshRequest {
        bibi_id: BibiId::new(),
        provider_id: ProviderId::new("1").unwrap(),
        stored_revision: None,
    }];
    provider.refresh_metadata(&requests).await;
    let items = provider
        .fetch_payloads(&[ProviderId::new("1").unwrap()])
        .await
        .unwrap();

    assert_eq!(
        items[0].payload.as_ref().unwrap().source_key().as_str(),
        "First:2012"
    );
    // Two requests total: the metadata pass already declared the texkeys, so
    // the join costs no extra lookup.
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn a_payload_fetch_without_a_prior_metadata_pass_looks_up_its_own_texkeys() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![record(1, "First:2012", None, None)])),
        Reply::ok(entry("First:2012")),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let items = provider
        .fetch_payloads(&[ProviderId::new("1").unwrap()])
        .await
        .unwrap();
    assert!(items[0].payload.is_some());
    assert_eq!(server.query(0), server.query(0));
    assert!(server.query(0).contains("fields=control_number,texkeys"));
}

#[tokio::test]
async fn a_record_that_received_no_entry_comes_back_absent_not_failed() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![
            record(1, "First:2012", None, None),
            record(2, "Second:2013", None, None),
        ])),
        Reply::ok(entry("First:2012")),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    let items = provider
        .fetch_payloads(&[ProviderId::new("1").unwrap(), ProviderId::new("2").unwrap()])
        .await
        .unwrap();
    assert!(items[0].payload.is_some());
    assert!(items[1].payload.is_none());
}

#[tokio::test]
async fn a_rate_limited_request_waits_the_floor_and_then_succeeds() {
    let clock = Arc::new(TestClock::new());
    let events: Arc<Mutex<Vec<RetryEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let server = TestServer::new(vec![
        // A one-second Retry-After is worse than useless: rejected requests
        // still cost quota, so bibi raises it to the five-second floor.
        Reply::rate_limited(Some("1")),
        Reply::ok(hits(vec![record(1, "First:2012", Some("1207.7214"), None)])),
        Reply::ok(entry("First:2012")),
    ]);
    let transport = Transport::builder()
        .base_url(&server.base_url)
        .clock(Arc::clone(&clock) as Arc<dyn bibi_inspire::Clock>)
        .on_retry(move |event| observed.lock().unwrap().push(event.clone()))
        .build()
        .unwrap();
    let provider = InspireProvider::with_transport(transport);

    let resolutions = provider
        .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
        .await
        .unwrap();
    assert!(matches!(resolutions[0], Resolution::Found(_)));
    assert_eq!(clock.waits(), [Duration::from_secs(5)]);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].attempt, 1);
    assert_eq!(events[0].delay, Duration::from_secs(5));
}

#[tokio::test]
async fn a_long_retry_after_is_capped_at_a_minute() {
    let clock = Arc::new(TestClock::new());
    let server = TestServer::new(vec![
        Reply::rate_limited(Some("3600")),
        Reply::ok(hits(vec![record(1, "First:2012", Some("1207.7214"), None)])),
        Reply::ok(entry("First:2012")),
    ]);
    let provider = provider(&server, Arc::clone(&clock));
    provider
        .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
        .await
        .unwrap();
    assert_eq!(clock.waits(), [Duration::from_secs(60)]);
}

#[tokio::test]
async fn exhausting_the_retries_is_a_retrieval_error() {
    let clock = Arc::new(TestClock::new());
    let server = TestServer::new(vec![
        Reply::rate_limited(None),
        Reply::rate_limited(None),
        Reply::rate_limited(None),
        Reply::rate_limited(None),
    ]);
    let provider = provider(&server, Arc::clone(&clock));
    let error = provider
        .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ProviderError::Retrieval(RetrievalError::RateLimited { attempts: 3, .. })
    ));
    // Three retries after the initial request, each at the floor.
    assert_eq!(clock.waits().len(), 3);
}

#[tokio::test]
async fn a_burst_of_requests_is_paced_under_the_documented_budget() {
    let clock = Arc::new(TestClock::new());
    // A two-per-second budget makes the schedule easy to read; the production
    // budget is twelve per five seconds.
    let limiter = Arc::new(RateLimiter::new(2, Duration::from_secs(1)));
    let replies = (0..6).map(|_| Reply::ok(hits(vec![]))).collect::<Vec<_>>();
    let server = TestServer::new(replies);
    let transport = Transport::builder()
        .base_url(&server.base_url)
        .clock(Arc::clone(&clock) as Arc<dyn bibi_inspire::Clock>)
        .limiter(limiter)
        .build()
        .unwrap();

    for _ in 0..6 {
        transport
            .search_json("control_number:1", 1, &[])
            .await
            .unwrap();
    }
    // Six requests at two per one-second window: the bucket binds twice, and
    // each wait frees a whole window's worth of permits rather than one.
    assert_eq!(
        clock.waits(),
        [Duration::from_secs(1), Duration::from_secs(1)]
    );
    assert!(
        clock.elapsed() >= Duration::from_secs(2),
        "paced under budget"
    );
}

#[tokio::test]
async fn the_inspire_provider_satisfies_the_shared_contract_suite() {
    let server = TestServer::new(vec![
        Reply::ok(hits(vec![record(1, "First:2012", Some("1207.7214"), None)])),
        Reply::ok(entry("First:2012")),
        Reply::ok(hits(vec![])),
    ]);
    let provider = provider(&server, Arc::new(TestClock::new()));
    verify_contract(
        &provider,
        &[
            Locator::Arxiv(ArxivId::new("1207.7214").unwrap()),
            Locator::Doi(Doi::new("10.1/absent").unwrap()),
        ],
    )
    .await;
}
