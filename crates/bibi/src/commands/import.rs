use super::{changed, open, persist, print_add_outcomes};
use anyhow::{Context, Result, bail};
use bibi_bibfile::{ConflictPolicy, Entry, FIELD_PREFIX, KeyRequest, PendingReference};
use bibi_bibliography::{scan_entries, strip_fields_with_prefix};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

/// Fold entries from other bibliographies into this one.
///
/// One-directional: sources are read and never written. Every source is
/// validated before anything is stored, so a malformed file cannot leave a
/// half-merged bibliography behind.
pub(crate) fn import(path: &Path, inputs: &[String], overwrite: bool) -> Result<()> {
    let mut file = open(path)?;
    let target = canonical(file.path());
    let mut pending = Vec::new();
    for input in inputs {
        if input != "-" && canonical(Path::new(input)) == target {
            bail!("refusing to import {} into itself", file.path().display());
        }
        // Scan rather than strictly parse: an incoming bibliography is somebody
        // else's working file and will have comments and directives in it. The
        // entries themselves are still validated, by projecting each one before
        // anything is stored.
        let source = read(input)?;
        for span in scan_entries(&source)? {
            let bibtex = source[span.span.clone()].to_owned();
            pending.push(PendingReference {
                key: KeyRequest::Exact(span.key.clone()),
                entry: Entry::new(span.key, bibtex),
            });
        }
    }
    let policy = if overwrite {
        ConflictPolicy::Overwrite
    } else {
        ConflictPolicy::Skip
    };
    let outcomes = file.add_batch(pending, policy)?;
    print_add_outcomes(&outcomes);
    if changed(&outcomes) {
        persist(&file)?;
    }
    Ok(())
}

/// Read one source with bibi's own field namespace stripped out.
///
/// Bookkeeping written by somebody else's bibi is not evidence about this
/// bibliography: a wrong record id would silently swap an entry for a different
/// paper at the next sync. Stripping is safe because `bibi sync` re-establishes
/// record ids from each entry's own identity.
fn read(input: &str) -> Result<String> {
    let source = if input == "-" {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .context("could not read BibTeX from stdin")?;
        source
    } else {
        fs::read_to_string(input).with_context(|| format!("could not read {input}"))?
    };
    Ok(strip_fields_with_prefix(&source, FIELD_PREFIX)?)
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
