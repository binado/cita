//! Generic provider fakes and contract verification.

use super::{MappingError, Provider, ProviderError, RetrievalError};
use crate::{Locator, ProviderName, RecordState};
use std::{collections::VecDeque, future::Future, sync::Mutex};

/// One call a fake provider received.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCall(pub Vec<String>);

/// A scriptable provider whose answers are complete batches.
pub struct FakeProvider {
    name: ProviderName,
    answers: Mutex<VecDeque<Result<Vec<RecordState>, ProviderError>>>,
    calls: Mutex<Vec<ProviderCall>>,
}

impl FakeProvider {
    /// Construct a fake for an installed provider.
    pub fn new(name: ProviderName) -> Self {
        Self {
            name,
            answers: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Queue one complete answer.
    pub fn answering(self, answer: Result<Vec<RecordState>, ProviderError>) -> Self {
        self.answers
            .lock()
            .expect("answer script")
            .push_back(answer);
        self
    }

    /// Queue a transport failure.
    pub fn failing(self, message: impl Into<String>) -> Self {
        let provider = self.name;
        self.answering(Err(RetrievalError::Transport {
            provider,
            message: message.into(),
        }
        .into()))
    }

    /// Calls received in order.
    pub fn calls(&self) -> Vec<ProviderCall> {
        self.calls.lock().expect("call log").clone()
    }
}

impl Provider for FakeProvider {
    fn name(&self) -> ProviderName {
        self.name
    }

    fn resolve(
        &self,
        locators: &[Locator],
    ) -> impl Future<Output = Result<Vec<RecordState>, ProviderError>> + Send {
        self.calls.lock().expect("call log").push(ProviderCall(
            locators.iter().map(ToString::to_string).collect(),
        ));
        let answer = self
            .answers
            .lock()
            .expect("answer script")
            .pop_front()
            .unwrap_or_else(|| {
                Err(ProviderError::contract(
                    self.name,
                    "fake received an unscripted call",
                ))
            });
        async move { answer }
    }
}

/// Verify the strict positional result-count contract for a successful lookup.
pub async fn verify_contract<P: Provider>(provider: &P, locators: &[Locator]) {
    let records = provider
        .resolve(locators)
        .await
        .expect("provider resolves batch");
    assert_eq!(
        records.len(),
        locators.len(),
        "provider must return exactly one state per locator"
    );
    for state in records {
        state.validate().expect("provider state is complete");
    }
}

/// Construct a contract-count failure at an adapter boundary.
pub fn count_mismatch(provider: ProviderName, expected: usize, actual: usize) -> ProviderError {
    MappingError::ContractViolation {
        provider,
        message: format!("expected {expected} results, received {actual}"),
    }
    .into()
}
