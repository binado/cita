//! The one test that reaches the real INSPIRE API.
//!
//! Ignored by default and run as a standalone liveness job: everything else in
//! the suite is hermetic. It asserts the shape of what comes back, never
//! volatile display text, so a corrected title upstream cannot fail CI.
//!
//! Run with: `cargo test -p bibi-inspire --test e2e -- --ignored`

use bibi_core::{
    ArxivId, BibiId, Locator, ProviderId,
    remote::{PayloadRequest, RefreshRequest, RefreshState, RemoteProvider, Resolution},
};
use bibi_inspire::InspireProvider;

/// The ATLAS Higgs discovery paper: public, stable, and unlikely to move.
const ARXIV: &str = "1207.7214";
const CONTROL_NUMBER: &str = "1124337";

#[tokio::test]
#[ignore = "requires network access to inspirehep.net"]
async fn resolves_a_stable_public_record_and_refreshes_it() {
    let provider = InspireProvider::new().expect("building the INSPIRE provider");

    let resolutions = provider
        .resolve(&[Locator::Arxiv(ArxivId::new(ARXIV).unwrap())])
        .await
        .expect("resolving a public record");
    let Resolution::Found(record) = &resolutions[0] else {
        panic!("expected to resolve {ARXIV}, got {:?}", resolutions[0]);
    };

    assert_eq!(record.provenance.provider.as_str(), "inspire");
    assert_eq!(
        record.provenance.provider_id.as_ref().unwrap().as_str(),
        CONTROL_NUMBER
    );
    assert!(
        record.provenance.revision.is_some(),
        "a record with no revision would be refetched on every sync"
    );
    assert_eq!(record.identifiers.arxiv.as_ref().unwrap().as_str(), ARXIV);
    assert!(record.identifiers.doi.is_some());
    assert!(!record.description.title.trim().is_empty());
    assert!(!record.description.authors.is_empty());
    assert!(
        record
            .description
            .collaborations
            .iter()
            .any(|collaboration| collaboration.contains("ATLAS")),
        "the structured record is where a collaboration survives"
    );

    // The payload is one well-formed entry whose key is one INSPIRE declares.
    assert!(record.payload.source().starts_with('@'));
    assert!(!record.payload.source_key().as_str().is_empty());

    // A refresh of the same record reports metadata rather than absence.
    let request = RefreshRequest {
        bibi_id: BibiId::new(),
        provider_id: ProviderId::new(CONTROL_NUMBER).unwrap(),
        stored_revision: None,
    };
    let items = provider.refresh_metadata(&[request]).await;
    let RefreshState::Metadata(metadata) = items[0]
        .result
        .as_ref()
        .expect("refreshing a public record")
    else {
        panic!("expected metadata, got {:?}", items[0].result);
    };
    assert_eq!(metadata.provider_id.as_str(), CONTROL_NUMBER);

    // A forced payload fetch pairs through the verified texkey join.
    let payloads = provider
        .fetch_payloads(&[PayloadRequest {
            provider_id: ProviderId::new(CONTROL_NUMBER).unwrap(),
            join_tokens: metadata.join_tokens.clone(),
        }])
        .await
        .expect("fetching a public payload");
    assert_eq!(payloads.len(), 1);
    assert!(
        payloads[0].payload.is_some(),
        "the join placed no entry for a record INSPIRE holds"
    );
}
