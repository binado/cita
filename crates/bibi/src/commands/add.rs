use super::{changed, inspire_client, open_or_create, persist, print_add_result};
use anyhow::{Result, bail};
use bibi_bibfile::{ConflictPolicy, Entry, KeyRequest, PendingReference};
use bibi_core::{Locator, MetadataProvider};
use std::path::Path;

pub(crate) async fn add(
    path: &Path,
    explicit_key: Option<&str>,
    values: &[String],
    overwrite: bool,
) -> Result<()> {
    if explicit_key.is_some() && values.len() != 1 {
        bail!("--key may only be used with one locator");
    }
    let locators = values
        .iter()
        .map(|value| value.parse::<Locator>())
        .collect::<Result<Vec<_>, _>>()?;
    // Load before resolving so a malformed bibliography fails without spending
    // a network round trip first; a missing one is simply a fresh start.
    let mut file = open_or_create(path)?;
    let client = inspire_client()?;
    let mut pending = Vec::with_capacity(locators.len());
    for locator in &locators {
        let record = client.resolve(locator).await?;
        let key = match explicit_key {
            Some(key) => KeyRequest::Exact(key.to_owned()),
            None => KeyRequest::Suggested(record.texkey.clone()),
        };
        // Build the complete entry before touching the file: a lookup that
        // fails must never leave a stub behind in a file the user owns.
        pending.push(PendingReference {
            entry: Entry::from_inspire(key.as_str(), &record)?,
            key,
        });
    }
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let outcomes = file.add_batch(pending, policy)?;
    print_add_result(&file, &outcomes);
    if changed(&outcomes) {
        persist(&file)?;
    }
    Ok(())
}
