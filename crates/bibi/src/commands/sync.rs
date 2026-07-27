use super::{inspire_client, open, persist};
use anyhow::Result;
use bibi_bibfile::Bibfile;
use std::path::Path;

pub(crate) async fn sync(path: &Path) -> Result<()> {
    let mut file = open(path)?;
    let outcome = refresh(&mut file).await?;
    println!("{outcome}");
    if outcome.refreshed > 0 {
        persist(&file)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SyncOutcome {
    /// Managed entries whose content actually changed.
    pub(crate) refreshed: usize,
    /// Managed entries considered.
    pub(crate) managed: usize,
    /// Entries bibi does not refresh.
    pub(crate) unmanaged: usize,
}

impl std::fmt::Display for SyncOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "refreshed {} of {} managed entries; {} unmanaged",
            self.refreshed, self.managed, self.unmanaged
        )
    }
}

/// Refresh every managed entry by its stable INSPIRE record id.
pub(crate) async fn refresh(file: &mut Bibfile) -> Result<SyncOutcome> {
    let managed = file.managed();
    let unmanaged = file.entries().len() - managed.len();
    if managed.is_empty() {
        return Ok(SyncOutcome {
            refreshed: 0,
            managed: 0,
            unmanaged,
        });
    }
    let ids = managed.iter().map(|(_, id)| *id).collect::<Vec<_>>();
    let records = inspire_client()?.refresh_records(&ids).await?;
    let refreshed = file.apply_records(&records)?;
    Ok(SyncOutcome {
        refreshed: refreshed.len(),
        managed: managed.len(),
        unmanaged,
    })
}
