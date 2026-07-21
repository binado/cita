mod add;
mod fetch;
mod generate;
mod import;
mod init;
mod list;
mod remove;
mod sync;

pub(crate) use add::add;
pub(crate) use fetch::fetch;
pub(crate) use generate::generate;
pub(crate) use import::import;
pub(crate) use init::init;
pub(crate) use list::list;
pub(crate) use remove::remove;
pub(crate) use sync::sync;

use anyhow::{Context, Result, bail};
use cita_inspire_client::{Client, RetryEvent};
use cita_manifest::{AddOutcome, MANIFEST_FILE};
use std::{
    env, fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
};

const CACHE_IGNORE_COMMENT: &str = "# cita document cache";
const CACHE_IGNORE_RULE: &str = "/.cita/files/";

pub(crate) fn find_manifest(start: &Path) -> Result<PathBuf> {
    for directory in start.ancestors() {
        let candidate = directory.join(MANIFEST_FILE);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!(
        "no cita.toml found in {} or its parents; run `cita init`",
        start.display()
    )
}

pub(crate) fn ensure_cache_layout(directory: &Path) -> Result<()> {
    let cache = directory.join(".cita/files");
    fs::create_dir_all(&cache)
        .with_context(|| format!("could not create document cache {}", cache.display()))?;
    let ignore_path = directory.join(".gitignore");
    let existing = match fs::read_to_string(&ignore_path) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| format!("could not read {}", ignore_path.display()));
        }
    };
    if existing
        .lines()
        .any(|line| line.trim() == CACHE_IGNORE_RULE)
    {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let mut ignore = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ignore_path)
        .with_context(|| format!("could not open {}", ignore_path.display()))?;
    write!(
        ignore,
        "{separator}{CACHE_IGNORE_COMMENT}\n{CACHE_IGNORE_RULE}\n"
    )
    .with_context(|| format!("could not update {}", ignore_path.display()))?;
    Ok(())
}

pub(crate) fn inspire_client() -> Result<Client> {
    let mut builder = Client::builder().on_retry(|event: &RetryEvent| {
        eprintln!(
            "INSPIRE rate limited the request for {}; retrying in {:?} (attempt {} of {})",
            event.resource, event.delay, event.attempt, event.max_retries
        );
    });
    if let Ok(base) = env::var("CITA_INSPIRE_BASE_URL") {
        builder = builder.base_url(base);
    }
    Ok(builder.build()?)
}

pub(crate) fn add_message(outcome: &AddOutcome) -> String {
    match outcome {
        AddOutcome::Added(key) => format!("Added {key}"),
        AddOutcome::Existing(key) => format!("Already present: {key}"),
        AddOutcome::Skipped { key, conflicting } if conflicting == key => {
            format!("Skipped {key}: local key already holds different content")
        }
        AddOutcome::Skipped { key, conflicting } => {
            format!("Skipped {key}: already present as {conflicting}")
        }
        AddOutcome::Overwritten {
            key,
            replaced: Some(old),
        } => format!("Overwrote {old} -> {key}"),
        AddOutcome::Overwritten {
            key,
            replaced: None,
        } => format!("Overwrote {key}"),
    }
}

pub(crate) fn print_add_outcomes(outcomes: &[AddOutcome]) {
    for outcome in outcomes {
        println!("{}", add_message(outcome));
    }
    let mut added = 0usize;
    let mut skipped = 0usize;
    let mut overwritten = 0usize;
    for outcome in outcomes {
        match outcome {
            AddOutcome::Added(_) => added += 1,
            AddOutcome::Skipped { .. } => skipped += 1,
            AddOutcome::Overwritten { .. } => overwritten += 1,
            AddOutcome::Existing(_) => {}
        }
    }
    // Keep a single clean add/import quiet; summarize only non-trivial batches.
    if outcomes.len() > 1 || skipped > 0 || overwritten > 0 {
        println!("{added} added, {skipped} skipped, {overwritten} overwritten");
    }
}
