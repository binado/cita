//! `bibi init`.

use crate::output;
use anyhow::Result;
use bibi_application::init;
use std::path::Path;

pub fn run(target: Option<&Path>) -> Result<bool> {
    let store = crate::bootstrap::store(target)?;
    init(&store)?;
    output::note(format!("created {}", store.path().display()));
    Ok(false)
}
