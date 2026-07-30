use bibi_core::{
    ArxivId, BibiId, Doi, Identifiers, ProviderId, QualifiedLocator, Revision,
    provider::{ProviderMetadata, RefreshState},
};
use bibi_provider::{
    Error, Provider, RefreshOptions, RefreshOutcome, RefreshTarget, ResolveItem,
    testing::{FakeProvider, ProviderCall, payload, provider_metadata, provider_record, providers},
};
use std::sync::Arc;

fn locators(values: &[&str]) -> Vec<QualifiedLocator> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

fn target(revision: Option<&str>) -> RefreshTarget {
    RefreshTarget {
        bibi_id: BibiId::new(),
        provider_id: ProviderId::new("1").unwrap(),
        stored_revision: revision.map(|value| Revision::new(value).unwrap()),
        identifiers: Identifiers::default(),
    }
}

#[tokio::test]
async fn resolution_defaults_to_inspire_and_uses_one_implementation() {
    let fake = Arc::new(FakeProvider::new("inspire").with_record(
        "arxiv:1207.7214",
        provider_record("inspire", "1", "Known:2012", "Known"),
    ));
    let facade = providers(fake.clone());
    let outcomes = facade
        .resolve(None, &locators(&["1207.7214", "10.1/missing"]))
        .await
        .unwrap();
    assert!(matches!(outcomes[0], ResolveItem::Found(_)));
    assert!(matches!(outcomes[1], ResolveItem::NotFound));
    assert_eq!(
        fake.calls(),
        [ProviderCall::Resolve(vec![
            "arxiv:1207.7214".into(),
            "doi:10.1/missing".into()
        ])]
    );
}

#[tokio::test]
async fn repeated_qualifiers_select_one_provider() {
    let fake = Arc::new(
        FakeProvider::new("inspire")
            .with_record("1", provider_record("inspire", "1", "One:2024", "One"))
            .with_record("2", provider_record("inspire", "2", "Two:2024", "Two")),
    );
    let outcomes = providers(fake)
        .resolve(None, &locators(&["inspire:1", "inspire:2"]))
        .await
        .unwrap();
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, ResolveItem::Found(_)))
    );
}

#[tokio::test]
async fn conflicts_mixed_qualifiers_unknown_names_and_local_fail_before_http() {
    let fake = Arc::new(FakeProvider::new("inspire"));
    let facade = providers(fake.clone());
    assert!(matches!(
        facade
            .resolve(Some(Provider::Local), &locators(&["inspire:1"]))
            .await,
        Err(Error::ProviderConflict { .. })
    ));
    assert!(matches!(
        facade
            .resolve(None, &locators(&["inspire:1", "local:2"]))
            .await,
        Err(Error::MixedProviders { .. })
    ));
    assert!(matches!(
        facade.resolve(None, &locators(&["ads:1"])).await,
        Err(Error::UnknownProvider { .. })
    ));
    assert!(matches!(
        facade
            .resolve(Some(Provider::Local), &locators(&["1207.7214"]))
            .await,
        Err(Error::LocalResolution)
    ));
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn absence_unsupported_and_failure_are_positional_and_never_fall_back() {
    let fake = Arc::new(
        FakeProvider::new("inspire")
            .unsupported("not-an-inspire-id")
            .failing_retrieval("down"),
    );
    let facade = providers(fake.clone());
    let outcomes = facade
        .resolve(None, &locators(&["1207.7214", "10.1/x"]))
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome, ResolveItem::Failed(_)))
    );
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn local_ingestion_is_explicit_and_preserves_payload() {
    let facade = providers(Arc::new(FakeProvider::new("inspire")));
    let source = "@misc{Mine, title={Mine}, note={  exact  }}";
    let entry = bibi_bibtex::BibtexEntry::parse_one(source.to_owned()).unwrap();
    let record = facade.ingest_local(entry).unwrap();
    assert_eq!(record.provenance.provider.as_str(), "local");
    assert!(record.provenance.provider_id.is_none());
    assert_eq!(record.payload.source(), source);
}

#[tokio::test]
async fn unchanged_refresh_does_not_fetch_payload_but_force_does() {
    let original_target = target(Some("r1"));
    let fake = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh(
                "1",
                RefreshState::Metadata(Box::new(provider_metadata("1", Some("r1"), "Same"))),
            )
            .with_payload("1", Some(payload("Remote:1", "Same"))),
    );
    let facade = providers(fake.clone());
    let first = facade
        .refresh(
            Provider::Inspire,
            std::slice::from_ref(&original_target),
            RefreshOptions::default(),
        )
        .await
        .unwrap();
    assert!(matches!(first[0].result, Ok(RefreshOutcome::Unchanged)));
    assert_eq!(
        fake.call_count(|call| matches!(call, ProviderCall::FetchPayloads(_))),
        0
    );

    let forced = facade
        .refresh(
            Provider::Inspire,
            &[original_target],
            RefreshOptions { force: true },
        )
        .await
        .unwrap();
    assert!(matches!(forced[0].result, Ok(RefreshOutcome::Updated(_))));
}

#[tokio::test]
async fn missing_payload_never_exposes_advanced_metadata() {
    let target = target(Some("old"));
    let fake = Arc::new(FakeProvider::new("inspire").with_refresh(
        "1",
        RefreshState::Metadata(Box::new(provider_metadata("1", Some("new"), "Changed"))),
    ));
    let result = providers(fake)
        .refresh(Provider::Inspire, &[target], RefreshOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        result[0].result,
        Ok(RefreshOutcome::PayloadMissing)
    ));
}

#[tokio::test]
async fn refresh_rejects_provider_id_and_identifier_replacement_but_merges_additions() {
    let mut wrong_id = provider_metadata("2", Some("r2"), "Changed");
    wrong_id.identifiers.doi = Some(Doi::new("10.1/new").unwrap());
    let id_target = target(Some("r1"));
    let fake = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh("1", RefreshState::Metadata(Box::new(wrong_id)))
            .with_payload("1", Some(payload("Remote:1", "Changed"))),
    );
    let failed = providers(fake)
        .refresh(Provider::Inspire, &[id_target], RefreshOptions::default())
        .await
        .unwrap();
    assert!(failed[0].result.is_err());

    let mut replacement_target = target(None);
    replacement_target.identifiers.doi = Some(Doi::new("10.1/old").unwrap());
    let mut replacement = provider_metadata("1", Some("r2"), "Changed");
    replacement.identifiers.doi = Some(Doi::new("10.1/new").unwrap());
    let fake = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh("1", RefreshState::Metadata(Box::new(replacement)))
            .with_payload("1", Some(payload("Remote:1", "Changed"))),
    );
    let failed = providers(fake)
        .refresh(
            Provider::Inspire,
            &[replacement_target],
            RefreshOptions::default(),
        )
        .await
        .unwrap();
    assert!(failed[0].result.is_err());

    let mut addition = provider_metadata("1", Some("r2"), "Changed");
    addition.identifiers.arxiv = Some(ArxivId::new("1207.7214").unwrap());
    let fake = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh("1", RefreshState::Metadata(Box::new(addition)))
            .with_payload("1", Some(payload("Remote:1", "Changed"))),
    );
    let updated = providers(fake)
        .refresh(
            Provider::Inspire,
            &[target(None)],
            RefreshOptions::default(),
        )
        .await
        .unwrap();
    let Ok(RefreshOutcome::Updated(record)) = &updated[0].result else {
        panic!("expected update");
    };
    assert_eq!(
        record.identifiers.arxiv,
        Some(ArxivId::new("1207.7214").unwrap())
    );
}

#[tokio::test]
async fn ambiguous_payload_failure_fails_the_provider_batch() {
    let metadata = |id: &str| {
        let mut value: ProviderMetadata = provider_metadata(id, Some("r2"), "Changed");
        value.provider_id = ProviderId::new(id).unwrap();
        RefreshState::Metadata(Box::new(value))
    };
    let first = RefreshTarget {
        provider_id: ProviderId::new("1").unwrap(),
        ..target(None)
    };
    let second = RefreshTarget {
        provider_id: ProviderId::new("2").unwrap(),
        ..target(None)
    };
    let fake = Arc::new(
        FakeProvider::new("inspire")
            .with_refresh("1", metadata("1"))
            .with_refresh("2", metadata("2"))
            .failing_payloads("ambiguous texkey join"),
    );
    let result = providers(fake)
        .refresh(
            Provider::Inspire,
            &[first, second],
            RefreshOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.iter().all(|item| item.result.is_err()));
}
