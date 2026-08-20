use super::{find_manifest, inspire_client, print_add_outcomes};
use anyhow::{Result, bail};
use cita_core::{Locator, MetadataProvider};
use cita_manifest::{ConflictPolicy, Entry, KeyRequest, Manifest, PendingReference};
use std::path::Path;

pub(crate) async fn add(
    cwd: &Path,
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
    let client = inspire_client()?;
    let mut pending = Vec::with_capacity(locators.len());
    for locator in &locators {
        let record = client.resolve(locator).await?;
        let key = match explicit_key {
            Some(key) => KeyRequest::Exact(key.to_owned()),
            None => KeyRequest::Suggested(record.texkey.clone()),
        };
        pending.push(PendingReference {
            key,
            entry: Entry::from_inspire(record)?,
        });
    }
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    let outcomes = manifest.add_batch(pending, policy)?;
    print_add_outcomes(&outcomes);
    Ok(())
}
