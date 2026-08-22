mod add;
mod edit;
mod export;
mod fetch;
mod generate;
mod import;
mod init;
mod library;
mod list;
mod remove;
mod sync;

pub(crate) use add::add;
pub(crate) use edit::edit;
pub(crate) use export::{export, export_outcome};
pub(crate) use fetch::{FetchOptions, fetch};
pub(crate) use generate::{generate, generate_outcome};
pub(crate) use import::import;
pub(crate) use init::init;
pub(crate) use library::{
    batch_export as library_export, batch_generate as library_generate, batch_sync as library_sync,
    init_library, init_shelf, list_shelves, resolve_target,
};
pub(crate) use list::list;
pub(crate) use remove::remove;
pub(crate) use sync::{sync, sync_outcome};

use anyhow::{Context, Result, bail};
use cita_inspire_client::{Client, RetryEvent};
use cita_manifest::{AddOutcome, MANIFEST_FILE};
use std::{
    env, fs,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
};

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

/// Bold-cyan identifier style, or None when the target stream should stay plain.
/// Matches the header styling in `list::print_rows` and honors NO_COLOR.
pub(crate) fn highlight_style(stream_is_terminal: bool) -> Option<anstyle::Style> {
    (stream_is_terminal && env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())).then(|| {
        anstyle::Style::new()
            .bold()
            .fg_color(Some(anstyle::AnsiColor::Cyan.into()))
    })
}

fn highlight(key: &str, style: Option<anstyle::Style>) -> String {
    match style {
        Some(s) => format!("{s}{key}{s:#}"),
        None => key.to_string(),
    }
}

pub(crate) fn add_message(outcome: &AddOutcome, style: Option<anstyle::Style>) -> String {
    match outcome {
        AddOutcome::Added(key) => format!("added {}", highlight(key, style)),
        AddOutcome::Existing(key) => format!("skipped {}", highlight(key, style)),
        AddOutcome::Skipped { key, conflicting } if conflicting == key => {
            format!(
                "skipped {}: local key already holds different content",
                highlight(key, style)
            )
        }
        AddOutcome::Skipped { key, conflicting } => {
            format!(
                "skipped {}: already present as {}",
                highlight(key, style),
                highlight(conflicting, style)
            )
        }
        AddOutcome::Overwritten { key, replaced } => match replaced.as_slice() {
            [] => format!("overwrote {}", highlight(key, style)),
            [only] if only == key => format!("overwrote {}", highlight(key, style)),
            [old] => format!(
                "overwrote {} -> {}",
                highlight(old, style),
                highlight(key, style)
            ),
            replaced => format!(
                "overwrote {} -> {}",
                replaced
                    .iter()
                    .map(|old| highlight(old, style))
                    .collect::<Vec<_>>()
                    .join(", "),
                highlight(key, style)
            ),
        },
    }
}

pub(crate) fn print_add_outcomes(outcomes: &[AddOutcome]) {
    let style = highlight_style(io::stdout().is_terminal());
    for outcome in outcomes {
        println!("{}", add_message(outcome, style));
    }
}
