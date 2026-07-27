use super::{inspire_client, open, persist};
use anyhow::Result;
use bibi_bibfile::Bibfile;
use bibi_inspire_client::Error as InspireError;
use std::{fmt, path::Path};

/// Reconcile the bibliography with INSPIRE.
///
/// Refreshing a managed entry and resolving an unmanaged one are the same
/// operation reached by different keys — one looks the record up by its stable
/// id, the other by the entry's own DOI or arXiv id and learns the id on the
/// way. Splitting them into two commands would have made the user responsible
/// for knowing which of their entries carries bookkeeping, which is exactly the
/// detail the tool should be hiding.
pub(crate) async fn sync(path: &Path, dry_run: bool, verbose: bool) -> Result<()> {
    let mut file = open(path)?;
    let report = reconcile(&mut file).await?;
    print!("{report}");
    report.print_details(verbose);
    if dry_run {
        eprintln!("Dry run: {} not written", file.path().display());
        return Ok(());
    }
    if report.changed() {
        persist(&file)?;
    }
    Ok(())
}

#[derive(Debug, Default)]
pub(crate) struct SyncReport {
    /// Unmanaged entries that INSPIRE recognized and bibi now tracks.
    adopted: Vec<String>,
    /// Managed entries whose content the provider actually changed.
    refreshed: Vec<String>,
    /// Managed entries considered.
    managed: usize,
    /// Entries with an identity INSPIRE does not know.
    unresolved: Vec<String>,
    /// Entries with nothing to look up at all.
    unidentified: usize,
    /// Entries the user marked off-limits.
    frozen: usize,
}

impl SyncReport {
    fn changed(&self) -> bool {
        !self.adopted.is_empty() || !self.refreshed.is_empty()
    }

    /// List what the summary only counted.
    fn print_details(&self, verbose: bool) {
        if !verbose {
            return;
        }
        for key in &self.adopted {
            println!("  adopted {key}");
        }
        for key in &self.refreshed {
            println!("  refreshed {key}");
        }
        for key in &self.unresolved {
            println!("  not on INSPIRE: {key}");
        }
    }
}

impl fmt::Display for SyncReport {
    /// Summarize rather than enumerate.
    ///
    /// A bibliography with textbooks and theses in it will always have entries
    /// INSPIRE cannot place, and listing them on every run would train the user
    /// to ignore the output.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.adopted.is_empty() {
            writeln!(formatter, "resolved {} entries", self.adopted.len())?;
        }
        writeln!(
            formatter,
            "refreshed {} of {} managed entries",
            self.refreshed.len(),
            self.managed
        )?;
        let unmanaged = self.unresolved.len() + self.unidentified;
        if unmanaged > 0 {
            writeln!(
                formatter,
                "{unmanaged} entries not on INSPIRE (--verbose to list)"
            )?;
        }
        if self.frozen > 0 {
            writeln!(formatter, "{} frozen entries left alone", self.frozen)?;
        }
        Ok(())
    }
}

async fn reconcile(file: &mut Bibfile) -> Result<SyncReport> {
    let managed = file.managed();
    let unmanaged = file.unmanaged()?;
    let frozen = file.entries().iter().filter(|e| e.is_frozen()).count();
    let mut report = SyncReport {
        managed: managed.len(),
        frozen,
        unidentified: file.entries().len() - managed.len() - unmanaged.len() - frozen,
        ..SyncReport::default()
    };
    if managed.is_empty() && unmanaged.is_empty() {
        return Ok(report);
    }
    let client = inspire_client()?;

    // Identity lookups first, so an entry adopted in this run is refreshed by
    // the same pass rather than waiting for the next one. Resolution records
    // the provider's current timestamp, so the refresh below finds them equal
    // and leaves the newly adopted entry alone.
    for (key, locator) in unmanaged {
        match client.resolve_snapshot(&locator).await {
            Ok(record) => {
                file.adopt(&key, &record)?;
                report.adopted.push(key);
            }
            // A bibliography legitimately holds work INSPIRE has never seen.
            // That is a fact to report, not a failure to abort on.
            Err(InspireError::NotFound(_)) => report.unresolved.push(key),
            Err(error) => return Err(error.into()),
        }
    }

    // Counted after adoption, so an entry resolved in this run is reported as
    // one of the managed entries the refresh below considered.
    let ids = file
        .managed()
        .into_iter()
        .map(|(_, id)| id)
        .collect::<Vec<_>>();
    report.managed = ids.len();
    if !ids.is_empty() {
        let records = client.refresh_records(&ids).await?;
        report.refreshed = file.apply_records(&records)?;
    }
    Ok(report)
}
