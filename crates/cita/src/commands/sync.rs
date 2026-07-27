use super::{Target, inspire_client};
use anyhow::Result;
use cita_core::MetadataProvider;
use std::fmt;

pub(crate) async fn sync(target: &Target) -> Result<()> {
    println!("{}", sync_outcome(target).await?);
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

pub(crate) async fn sync_outcome(target: &Target) -> Result<SyncOutcome> {
    let _lock = target.lock()?;
    let mut manifest = target.load()?;
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
