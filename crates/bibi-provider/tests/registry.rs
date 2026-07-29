//! Registry dispatch: roster order, group fallback, and arity validation.

use bibi_core::{Locator, ProviderId, ProviderName, QualifiedLocator};
use bibi_provider::{
    LocalProvider, LocatorOutcome, PayloadRequest, Provider, ProviderCapabilities, ProviderError,
    ProviderFuture, ProviderRegistry, RefreshItem, RefreshRequest, RegistryError, Resolution,
    testing::{FakeProvider, ProviderCall, provider_record, verify_contract},
};
use std::sync::Arc;

fn name(value: &str) -> ProviderName {
    ProviderName::new(value).unwrap()
}

fn locators(values: &[&str]) -> Vec<QualifiedLocator> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

fn registry(providers: Vec<Arc<dyn Provider>>) -> ProviderRegistry {
    ProviderRegistry::new(providers)
}

#[tokio::test]
async fn the_first_provider_in_roster_order_that_resolves_wins() {
    let first = Arc::new(FakeProvider::new("first").with_record(
        "arxiv:1207.7214",
        provider_record("first", "1", "First:2012", "From first"),
    ));
    let second = Arc::new(FakeProvider::new("second").with_record(
        "arxiv:1207.7214",
        provider_record("second", "2", "Second:2012", "From second"),
    ));
    let registry = registry(vec![first.clone(), second.clone()]);
    let outcomes = registry
        .resolve(&locators(&["1207.7214"]), None)
        .await
        .unwrap();
    let LocatorOutcome::Found { provider, record } = &outcomes[0] else {
        panic!("expected a record, got {:?}", outcomes[0]);
    };
    assert_eq!(provider.as_str(), "first");
    assert_eq!(record.description.title, "From first");
    // The second provider is never asked about a locator already resolved.
    assert!(second.calls().is_empty());
}

#[tokio::test]
async fn absence_advances_to_the_next_provider_as_a_subset() {
    let first = Arc::new(FakeProvider::new("first").with_record(
        "arxiv:1207.7214",
        provider_record("first", "1", "First:2012", "Known to first"),
    ));
    let second = Arc::new(FakeProvider::new("second").with_record(
        "doi:10.1/b",
        provider_record("second", "2", "Second:2012", "Known to second"),
    ));
    let registry = registry(vec![first.clone(), second.clone()]);
    let outcomes = registry
        .resolve(&locators(&["1207.7214", "10.1/b", "10.1/missing"]), None)
        .await
        .unwrap();

    assert!(
        matches!(&outcomes[0], LocatorOutcome::Found { provider, .. } if provider.as_str() == "first")
    );
    assert!(
        matches!(&outcomes[1], LocatorOutcome::Found { provider, .. } if provider.as_str() == "second")
    );
    assert!(matches!(outcomes[2], LocatorOutcome::NotFound));

    // One call each, and the second only sees what the first could not place.
    assert_eq!(
        first.calls(),
        [ProviderCall::Resolve(vec![
            "arxiv:1207.7214".into(),
            "doi:10.1/b".into(),
            "doi:10.1/missing".into()
        ])]
    );
    assert_eq!(
        second.calls(),
        [ProviderCall::Resolve(vec![
            "doi:10.1/b".into(),
            "doi:10.1/missing".into()
        ])]
    );
}

#[tokio::test]
async fn a_provider_error_stops_fallback_for_the_locators_it_carried() {
    let failing = Arc::new(FakeProvider::new("first").failing_retrieval("connection reset"));
    let second = Arc::new(FakeProvider::new("second").with_record(
        "arxiv:1207.7214",
        provider_record("second", "2", "Second:2012", "Would have resolved"),
    ));
    let registry = registry(vec![failing, second.clone()]);
    let outcomes = registry
        .resolve(&locators(&["1207.7214"]), None)
        .await
        .unwrap();

    let LocatorOutcome::Failed { provider, error } = &outcomes[0] else {
        panic!("expected a failure, got {:?}", outcomes[0]);
    };
    assert_eq!(provider.as_str(), "first");
    assert!(error.to_string().contains("connection reset"));
    // A temporary outage must not silently hand provenance to another provider.
    assert!(second.calls().is_empty());
}

#[tokio::test]
async fn one_failed_call_fails_every_locator_it_carried() {
    let failing = Arc::new(FakeProvider::new("first").failing_mapping("unparseable date"));
    let registry = registry(vec![failing]);
    let outcomes = registry
        .resolve(&locators(&["1207.7214", "10.1/b"]), None)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 2);
    for outcome in &outcomes {
        let LocatorOutcome::Failed { error, .. } = outcome else {
            panic!("expected a failure, got {outcome:?}");
        };
        assert!(error.to_string().contains("unparseable date"));
    }
}

#[tokio::test]
async fn a_qualified_locator_selects_exactly_one_provider() {
    let first = Arc::new(FakeProvider::new("first").with_record(
        "1124337",
        provider_record("first", "1124337", "First:2012", "From first"),
    ));
    let second = Arc::new(FakeProvider::new("second").with_record(
        "1124337",
        provider_record("second", "1124337", "Second:2012", "From second"),
    ));
    let registry = registry(vec![first.clone(), second.clone()]);
    let outcomes = registry
        .resolve(&locators(&["second:1124337"]), None)
        .await
        .unwrap();
    assert!(
        matches!(&outcomes[0], LocatorOutcome::Found { provider, .. } if provider.as_str() == "second")
    );
    assert!(first.calls().is_empty());
}

#[tokio::test]
async fn a_qualifier_disagreeing_with_the_flag_fails_before_any_request() {
    let first = Arc::new(FakeProvider::new("first"));
    let second = Arc::new(FakeProvider::new("second"));
    let registry = registry(vec![first.clone(), second.clone()]);
    let error = registry
        .resolve(&locators(&["second:1"]), Some(&name("first")))
        .await
        .unwrap_err();
    assert!(matches!(error, RegistryError::ProviderConflict { .. }));
    assert!(first.calls().is_empty() && second.calls().is_empty());
}

#[tokio::test]
async fn naming_an_uninstalled_provider_is_a_usage_error() {
    let registry = registry(vec![Arc::new(FakeProvider::new("first"))]);
    assert!(matches!(
        registry
            .resolve(&locators(&["1207.7214"]), Some(&name("ads")))
            .await,
        Err(RegistryError::UnknownProvider { .. })
    ));
    assert!(matches!(
        registry.resolve(&locators(&["ads:123"]), None).await,
        Err(RegistryError::UnknownProvider { .. })
    ));
}

#[tokio::test]
async fn a_bare_provider_id_needs_exactly_one_claimant() {
    let first = Arc::new(
        FakeProvider::new("first")
            .recognizing("1124337")
            .with_record(
                "1124337",
                provider_record("first", "1124337", "First:2012", "Recognized"),
            ),
    );
    let second = Arc::new(FakeProvider::new("second").recognizing("1124337"));
    let unrelated = Arc::new(FakeProvider::new("third"));

    let one = registry(vec![first.clone(), unrelated.clone()]);
    assert!(matches!(
        one.resolve(&locators(&["1124337"]), None).await.unwrap()[0],
        LocatorOutcome::Found { .. }
    ));

    let both = registry(vec![first, second]);
    let outcomes = both.resolve(&locators(&["1124337"]), None).await.unwrap();
    let LocatorOutcome::Ambiguous { providers } = &outcomes[0] else {
        panic!("expected ambiguity, got {:?}", outcomes[0]);
    };
    assert_eq!(providers.len(), 2);

    let none = registry(vec![unrelated]);
    assert!(matches!(
        none.resolve(&locators(&["1124337"]), None).await.unwrap()[0],
        LocatorOutcome::Unrecognized
    ));
}

#[tokio::test]
async fn a_provider_that_answers_the_wrong_number_of_locators_is_refused() {
    struct Miscounting(ProviderName);

    impl Provider for Miscounting {
        fn name(&self) -> &ProviderName {
            &self.0
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::NETWORK
        }
        fn recognizes_unqualified_id(&self, _: &str) -> bool {
            false
        }
        fn resolve<'a>(
            &'a self,
            _: &'a [Locator],
        ) -> ProviderFuture<'a, Result<Vec<Resolution>, ProviderError>> {
            // One answer for two questions: a caller could read one record's
            // result as another's.
            Box::pin(async { Ok(vec![Resolution::NotFound]) })
        }
        fn refresh_metadata<'a>(
            &'a self,
            _: &'a [RefreshRequest],
        ) -> ProviderFuture<'a, Vec<RefreshItem>> {
            Box::pin(async { Vec::new() })
        }
        fn fetch_payloads<'a>(
            &'a self,
            _: &'a [PayloadRequest],
        ) -> ProviderFuture<'a, Result<Vec<bibi_provider::PayloadItem>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }
        fn ingest(&self, _: bibi_bibtex::BibtexEntry) -> Result<Resolution, ProviderError> {
            Ok(Resolution::UnsupportedLocator)
        }
    }

    let provider: Arc<dyn Provider> = Arc::new(Miscounting(name("miscounting")));
    let registry = registry(vec![provider.clone()]);
    let outcomes = registry
        .resolve(&locators(&["1207.7214", "10.1/b"]), None)
        .await
        .unwrap();
    for outcome in &outcomes {
        assert!(
            matches!(outcome, LocatorOutcome::Failed { .. }),
            "{outcome:?}"
        );
    }

    // The same check guards refresh results.
    let requests = [RefreshRequest {
        bibi_id: bibi_core::BibiId::new(),
        provider_id: ProviderId::new("1").unwrap(),
        stored_revision: None,
    }];
    assert!(
        registry
            .refresh_metadata(&provider, &requests)
            .await
            .is_err()
    );
    assert!(
        registry
            .fetch_payloads(
                &provider,
                &[PayloadRequest {
                    provider_id: ProviderId::new("1").unwrap(),
                    join_tokens: Vec::new(),
                }]
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_record_claiming_another_providers_name_is_refused() {
    let liar = Arc::new(FakeProvider::new("first").with_record(
        "arxiv:1207.7214",
        provider_record("second", "1", "Key:2012", "Owned by someone else"),
    ));
    let registry = registry(vec![liar]);
    let outcomes = registry
        .resolve(&locators(&["1207.7214"]), None)
        .await
        .unwrap();
    assert!(matches!(outcomes[0], LocatorOutcome::Failed { .. }));
}

#[tokio::test]
async fn an_ingest_only_provider_is_never_offered_a_locator() {
    let local = Arc::new(LocalProvider::new());
    let network = Arc::new(FakeProvider::new("network").with_record(
        "arxiv:1207.7214",
        provider_record("network", "1", "Net:2012", "Resolved"),
    ));
    let registry = registry(vec![network, local.clone()]);
    let outcomes = registry
        .resolve(&locators(&["1207.7214", "10.1/absent"]), None)
        .await
        .unwrap();
    assert!(matches!(outcomes[0], LocatorOutcome::Found { .. }));
    // Absence stays absence: the local provider is not a fallback resolver, and
    // creating a local record is the application's decision, not the registry's.
    assert!(matches!(outcomes[1], LocatorOutcome::NotFound));
}

#[test]
fn capability_queries_replace_asking_for_a_providers_name() {
    let registry = registry(vec![
        Arc::new(FakeProvider::new("network")),
        Arc::new(LocalProvider::new()),
    ]);
    assert_eq!(
        registry.unrefreshable_names(),
        [ProviderName::new("local").unwrap()]
    );
    assert_eq!(
        registry
            .ingest_provider()
            .map(|provider| provider.name().as_str()),
        Some("local")
    );
    assert_eq!(
        registry
            .names()
            .iter()
            .map(ProviderName::as_str)
            .collect::<Vec<_>>(),
        ["network", "local"]
    );
}

#[tokio::test]
async fn the_bundled_providers_satisfy_the_contract_suite() {
    let probes = [
        Locator::Arxiv(bibi_core::ArxivId::new("1207.7214").unwrap()),
        Locator::Doi(bibi_core::Doi::new("10.1/x").unwrap()),
        Locator::ProviderId("1124337".into()),
    ];
    verify_contract(&LocalProvider::new(), &probes).await;
    verify_contract(
        &FakeProvider::new("network").with_record(
            "arxiv:1207.7214",
            provider_record("network", "1", "Net:2012", "Resolved"),
        ),
        &probes,
    )
    .await;
}
