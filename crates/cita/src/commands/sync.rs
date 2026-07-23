use super::{find_manifest, inspire_client};
use anyhow::Result;
use cita_core::MetadataProvider;
use cita_manifest::Manifest;
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

impl SyncOutcome {
    fn synced_message(self, lead: &str, include_left: bool) -> String {
        if include_left {
            format!(
                "{lead} {} managed references; left {} imported unchanged",
                self.managed, self.imported
            )
        } else {
            format!(
                "{lead} {} managed references; {} imported unchanged",
                self.managed, self.imported
            )
        }
    }

    fn already_message(self, lead: &str) -> String {
        format!(
            "{lead}: {} managed, {} imported",
            self.managed, self.imported
        )
    }

    pub(crate) fn batch_message(self) -> String {
        if self.changed {
            self.synced_message("synced", false)
        } else {
            self.already_message("already in sync")
        }
    }
}

impl fmt::Display for SyncOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}",
            if self.changed {
                self.synced_message("Synced", true)
            } else {
                self.already_message("Already in sync")
            }
        )
    }
}

pub(crate) async fn sync_outcome(cwd: &Path) -> Result<SyncOutcome> {
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
