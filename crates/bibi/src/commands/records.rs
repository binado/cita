//! `bibi show`, `bibi remove`, and `bibi rename`.

use crate::{
    cli::{RemoveArgs, RenameArgs, ShowArgs},
    output,
};
use anyhow::Result;
use bibi_application::domain::CitationKey;
use bibi_application::{Services, TargetSelection, remove, rename, show};

pub fn run_show(services: &Services, target: &TargetSelection, args: ShowArgs) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    output::emit(&show(&store, &args.selector)?)?;
    Ok(false)
}

pub fn run_remove(services: &Services, target: &TargetSelection, args: RemoveArgs) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    let report = remove(&store, &args.selectors, args.dry_run)?;
    // What was removed goes to stdout, which makes it recovery input: piping it
    // back through `add -f` restores the records.
    let entries = report
        .items
        .successes
        .iter()
        .map(|removed| removed.bibtex.clone())
        .collect::<Vec<_>>();
    if !entries.is_empty() {
        output::emit(&(entries.join("\n\n") + "\n"))?;
    }
    output::report(&report.items);
    if args.dry_run {
        output::note(format!(
            "dry run: would remove {}",
            report.items.successes.len()
        ));
    } else if report.committed {
        output::note(format!("removed {}", report.items.successes.len()));
    }
    Ok(report.items.has_failures())
}

pub fn run_rename(services: &Services, target: &TargetSelection, args: RenameArgs) -> Result<bool> {
    let store = crate::bootstrap::store(services, target)?;
    let renamed = rename(&store, &args.selector, &CitationKey::new(args.key)?)?;
    output::emit(&(renamed.rendered()? + "\n"))?;
    output::note(format!(
        "renamed to `{}`; update any `\\cite{{}}` uses yourself",
        renamed.key
    ));
    Ok(false)
}
