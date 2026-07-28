use super::{Target, inspire_client, print_add_outcomes};
use anyhow::{Result, bail};
use cita_core::Locator;
use cita_store::{ConflictPolicy, KeyRequest, PendingReference, SourceSnapshot};

pub(crate) async fn add(
    target: &Target<'_>,
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
        let record = client.resolve_snapshot(locator).await?;
        let key = match explicit_key {
            Some(key) => KeyRequest::Exact(key.to_owned()),
            None => KeyRequest::Suggested(record.texkey.clone()),
        };
        pending.push(PendingReference {
            key,
            source: SourceSnapshot::inspire(record),
        });
    }
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let outcomes = target.library().add_batch(target.name(), pending, policy)?;
    print_add_outcomes(&outcomes);
    Ok(())
}
