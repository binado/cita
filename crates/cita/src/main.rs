mod git;

use anyhow::{Context, Result, bail};
use cita_core::{Locator, MetadataProvider};
use cita_inspire_client::InspireProvider;
use cita_manifest::{AddOutcome, Manifest, export_bibtex};
use clap::{CommandFactory, Parser, Subcommand};
use std::{
    env,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(name = "cita", version, about = "A Git-friendly bibliography database")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create cita.toml at the repository root or current directory
    Init,
    /// Resolve and add one or more papers through INSPIRE
    Add {
        /// Override the citation key (only valid with one locator)
        #[arg(long)]
        key: Option<String>,
        /// arXiv id, or an arxiv:, doi:, or inspire: locator
        #[arg(required = true)]
        locators: Vec<String>,
    },
    /// Remove papers by citation key or locator
    Remove {
        #[arg(required = true)]
        selectors: Vec<String>,
    },
    /// List stored papers
    List,
    /// Export the committed manifest as a deterministic format
    Export {
        /// Write BibTeX to standard output
        #[arg(long, required = true)]
        bibtex: bool,
    },
    /// Commit only cita.toml to Git
    Commit,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir().context("could not determine current directory")?;
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
        }
        Some(Command::Init) => init(&cwd)?,
        Some(Command::Add { key, locators }) => add(&cwd, key.as_deref(), &locators).await?,
        Some(Command::Remove { selectors }) => remove(&cwd, &selectors)?,
        Some(Command::List) => list(&cwd)?,
        Some(Command::Export { bibtex: true }) => export(&cwd)?,
        Some(Command::Export { bibtex: false }) => unreachable!("clap requires --bibtex"),
        Some(Command::Commit) => git::commit(&find_manifest(&cwd)?)?,
    }
    Ok(())
}

fn init(cwd: &Path) -> Result<()> {
    let directory = git::repository_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let path = directory.join("cita.toml");
    Manifest::create(&path)?;
    println!("Initialized {}", path.display());
    Ok(())
}

async fn add(cwd: &Path, key: Option<&str>, values: &[String]) -> Result<()> {
    if key.is_some() && values.len() != 1 {
        bail!("--key can only be used with one locator");
    }
    let path = find_manifest(cwd)?;
    let mut manifest = Manifest::load(&path)?;
    let provider = InspireProvider::new()?;
    let outcomes = if let Some(key) = key {
        let locator = values[0].parse::<Locator>()?;
        let paper = provider.resolve(&locator).await?;
        vec![manifest.add(paper, Some(key))?]
    } else {
        let mut resolved = Vec::with_capacity(values.len());
        for value in values {
            let locator = value.parse::<Locator>()?;
            resolved.push(provider.resolve(&locator).await?);
        }
        manifest.add_batch(resolved)?
    };
    for outcome in outcomes {
        match outcome {
            AddOutcome::Added(key) => println!("Added {key}"),
            AddOutcome::Existing(key) => println!("Already present: {key}"),
        }
    }
    Ok(())
}

fn remove(cwd: &Path, selectors: &[String]) -> Result<()> {
    let path = find_manifest(cwd)?;
    let mut manifest = Manifest::load(&path)?;
    for (key, _) in manifest.remove_batch(selectors)? {
        println!("Removed {key}");
    }
    Ok(())
}

fn list(cwd: &Path) -> Result<()> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    if manifest.papers().is_empty() {
        return Ok(());
    }
    let rows = manifest
        .papers()
        .iter()
        .map(|(key, record)| {
            let author = record
                .authors
                .first()
                .or_else(|| record.collaborations.first())
                .map(String::as_str)
                .unwrap_or("—");
            [
                key.clone(),
                record.year.map_or_else(|| "—".into(), |y| y.to_string()),
                author.into(),
                record.title.clone(),
            ]
        })
        .collect::<Vec<_>>();
    let headers = ["KEY", "YEAR", "AUTHOR/COLLABORATION", "TITLE"];
    // Only the first three columns are padded; the trailing title column is
    // printed unpadded, so its width never needs to be tracked.
    let mut widths = [0usize; 3];
    for (width, header) in widths.iter_mut().zip(headers) {
        *width = header.chars().count();
    }
    for row in &rows {
        for (width, value) in widths.iter_mut().zip(&row[..3]) {
            *width = (*width).max(value.chars().count());
        }
    }
    println!(
        "{:<w0$}  {:<w1$}  {:<w2$}  {}",
        headers[0],
        headers[1],
        headers[2],
        headers[3],
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2]
    );
    for row in rows {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2]
        );
    }
    Ok(())
}

fn export(cwd: &Path) -> Result<()> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    let output = export_bibtex(manifest.papers());
    std::io::stdout().write_all(output.as_bytes())?;
    Ok(())
}

fn find_manifest(start: &Path) -> Result<PathBuf> {
    for directory in start.ancestors() {
        let candidate = directory.join("cita.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!(
        "no cita.toml found in {} or its parents; run `cita init`",
        start.display()
    )
}
