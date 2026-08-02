use bibi_core::{ArxivId, Locator, ProviderName, remote::testing::FakeProvider};
use bibi_provider::{
    Error, ProviderError, Providers,
    testing::{provider_record, providers},
};
use std::sync::Arc;

#[tokio::test]
async fn facade_returns_one_complete_positional_state_per_locator() {
    let states = vec![
        provider_record("1", "First", "First title"),
        provider_record("2", "Second", "Second title"),
    ];
    let fake = Arc::new(FakeProvider::new(ProviderName::Inspire).answering(Ok(states)));
    let facade = providers(fake.clone());
    let locators = [
        Locator::Arxiv(ArxivId::new("1207.7214").unwrap()),
        Locator::Opaque("2".into()),
    ];
    let found = facade.resolve(None, &locators).await.unwrap();
    assert_eq!(
        found.iter().map(|state| state.texkey()).collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert_eq!(fake.calls()[0].0, ["arxiv:1207.7214", "2"]);
}

#[tokio::test]
async fn count_mismatch_and_provider_failure_fail_the_whole_call() {
    let short = providers(Arc::new(
        FakeProvider::new(ProviderName::Inspire).answering(Ok(Vec::new())),
    ));
    let error = short
        .resolve(None, &[Locator::Opaque("1".into())])
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Provider(ProviderError::Mapping(_))));

    let failed = providers(Arc::new(
        FakeProvider::new(ProviderName::Inspire).failing("offline"),
    ));
    assert!(
        failed
            .resolve(None, &[Locator::Opaque("1".into())])
            .await
            .is_err()
    );
}

#[test]
fn provider_roster_is_closed() {
    assert_eq!(
        Providers::installed("inspire").unwrap(),
        ProviderName::Inspire
    );
    assert!(Providers::installed("local").is_err());
}
