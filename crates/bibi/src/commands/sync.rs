use super::{find_manifest, inspire_client};
use anyhow::Result;
use bibi_core::MetadataProvider;
use bibi_manifest::Manifest;
use std::{fmt, path::Path};

pub(crate) async fn sync(cwd: &Path) -> Result<()> {
    println!("{}", sync_outcome(cwd).await?);
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SyncOutcome {
    changed: bool,
    managed: usize,
    imported: usize,
}

impl fmt::Display for SyncOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.changed {
            write!(
                formatter,
                "Synced {} managed references; left {} imported unchanged",
                self.managed, self.imported
            )
        } else {
            write!(
                formatter,
                "Already in sync: {} managed, {} imported",
                self.managed, self.imported
            )
        }
    }
}

async fn sync_outcome(cwd: &Path) -> Result<SyncOutcome> {
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    let ids = manifest.inspire_record_ids();
    let managed = ids.len();
    let imported = manifest.references().len() - managed;
    let provider_ids = ids.iter().map(u64::to_string).collect::<Vec<_>>();
    let refreshed = inspire_client()?.refresh(&provider_ids).await?;
    let changed = manifest.replace_inspire(refreshed)?;
    Ok(SyncOutcome {
        changed,
        managed,
        imported,
    })
}
