//! `bibi init`.

use crate::output;
use anyhow::Result;
use bibi_application::{Services, TargetSelection, init};

pub fn run(services: &Services, target: &TargetSelection) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    init(&store)?;
    output::note(format!("created {}", store.path().display()));
    Ok(false)
}
