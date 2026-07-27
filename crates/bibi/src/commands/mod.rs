mod add;
mod check;
mod export;
mod fetch;
mod import;
mod list;
mod rekey;
mod remove;
mod sync;

pub(crate) use add::add;
pub(crate) use check::check;
pub(crate) use export::export;
pub(crate) use fetch::{FetchOptions, fetch};
pub(crate) use import::import;
pub(crate) use list::list;
pub(crate) use rekey::rekey;
pub(crate) use remove::remove;
pub(crate) use sync::sync;

use anyhow::{Context, Result};
use bibi_bibfile::{AddOutcome, Bibfile};
use bibi_inspire_client::{Client, RetryEvent};
use std::{
    env, fs,
    fs::OpenOptions,
    io::{self, IsTerminal, Write},
    path::Path,
};

const CACHE_IGNORE_COMMENT: &str = "# bibi document cache";
const CACHE_IGNORE_RULE: &str = "/.bibi/files/";

/// Load the bibliography a command was pointed at.
pub(crate) fn open(path: &Path) -> Result<Bibfile> {
    Ok(Bibfile::load(path)?)
}

/// Write a mutated bibliography and report which file changed.
///
/// Saying so matters more than it used to: without a walk up the tree there is
/// no single legal target, so a mutation that stayed quiet about where it wrote
/// would be a trap in any directory holding more than one bibliography.
pub(crate) fn persist(file: &Bibfile) -> Result<()> {
    file.write()?;
    eprintln!("Wrote {}", file.path().display());
    Ok(())
}

/// The directory whose `.bibi/files` cache serves this bibliography.
pub(crate) fn cache_root(file: &Bibfile) -> &Path {
    file.path().parent().unwrap_or(Path::new("."))
}

pub(crate) fn ensure_cache_layout(directory: &Path) -> Result<()> {
    let cache = directory.join(".bibi/files");
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
    if let Ok(base) = env::var("BIBI_INSPIRE_BASE_URL") {
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

/// Whether any outcome actually changed the file.
pub(crate) fn changed(outcomes: &[AddOutcome]) -> bool {
    outcomes.iter().any(|outcome| {
        matches!(
            outcome,
            AddOutcome::Added(_) | AddOutcome::Overwritten { .. }
        )
    })
}
