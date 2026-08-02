//! Live INSPIRE liveness. Deliberately ignored in the hermetic suite.

use bibi_core::{ArxivId, Locator, ProviderName, Source, remote::Provider};
use bibi_inspire::InspireProvider;

#[tokio::test]
#[ignore = "requires the public INSPIRE API"]
async fn resolves_one_complete_current_state() {
    let provider = InspireProvider::new().unwrap();
    let states = provider
        .resolve(&[Locator::Arxiv(ArxivId::new("1207.7214").unwrap())])
        .await
        .unwrap();
    assert_eq!(states.len(), 1);
    assert!(matches!(
        states[0].source(),
        Source::Managed {
            provider: ProviderName::Inspire,
            ..
        }
    ));
    assert!(!states[0].texkey().is_empty());
}
