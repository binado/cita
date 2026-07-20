mod commands;
mod git;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use std::env;

#[derive(Debug, Parser)]
#[command(name = "cita", version, about = "A Git-friendly bibliography database")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize cita.toml and its generated references.bib
    Init,
    /// Import standalone BibTeX entries from a path or stdin (`-`)
    Import { path: String },
    /// Resolve and add one or more references through INSPIRE
    Add {
        /// Keep this local citation key (one locator only)
        #[arg(long)]
        key: Option<String>,
        #[arg(required = true)]
        locators: Vec<String>,
    },
    /// Refresh every INSPIRE-managed source snapshot by stable record id
    Sync,
    /// Remove references by local key or provider/DOI/arXiv identity
    Remove {
        #[arg(required = true)]
        selectors: Vec<String>,
    },
    /// List stored references
    List {
        #[arg(long, value_enum, default_value_t = SortBy::Key)]
        sort_by: SortBy,
        #[arg(long, value_enum, default_value_t = Order::Asc)]
        order: Order,
        #[arg(long)]
        no_wrap_title: bool,
    },
    /// Regenerate a missing or edited references.bib
    Generate,
    /// Fetch or resolve a reference's arXiv PDF
    Fetch {
        #[arg(long, conflicts_with_all = ["cache_only", "url"])]
        force: bool,
        /// Require an existing cached PDF without downloading
        #[arg(long, conflicts_with = "url")]
        cache_only: bool,
        /// Return the arXiv PDF URL without downloading
        #[arg(short = 'u', long)]
        url: bool,
        /// Open the returned path or URL with the system default application
        #[arg(long)]
        open: bool,
        #[arg(long)]
        save: bool,
        selector: String,
    },
    /// Commit cita.toml and references.bib, leaving unrelated files alone
    Commit,
}

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum SortBy {
    #[default]
    Key,
    Title,
    Author,
    Year,
}
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum Order {
    #[default]
    Asc,
    Desc,
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
        Some(Command::Init) => commands::init(&cwd)?,
        Some(Command::Import { path }) => commands::import(&cwd, &path)?,
        Some(Command::Add { key, locators }) => {
            commands::add(&cwd, key.as_deref(), &locators).await?
        }
        Some(Command::Sync) => commands::sync(&cwd).await?,
        Some(Command::Remove { selectors }) => commands::remove(&cwd, &selectors)?,
        Some(Command::List {
            sort_by,
            order,
            no_wrap_title,
        }) => commands::list(&cwd, sort_by, order, !no_wrap_title)?,
        Some(Command::Generate) => commands::generate(&cwd)?,
        Some(Command::Fetch {
            force,
            cache_only,
            url,
            open,
            save,
            selector,
        }) => commands::fetch(&cwd, &selector, force, cache_only, url, open, save).await?,
        Some(Command::Commit) => git::commit(&commands::find_manifest(&cwd)?)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_only_accepts_one_locator() {
        assert!(Cli::try_parse_from(["cita", "add", "--key", "X", "1207.7214"]).is_ok());
    }
    #[test]
    fn document_conflicts_are_enforced() {
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--force", "--cache-only", "1207.7214"]).is_err()
        );
        assert!(Cli::try_parse_from(["cita", "fetch", "--url", "--force", "1207.7214"]).is_err());
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--url", "--cache-only", "1207.7214"]).is_err()
        );
        assert!(Cli::try_parse_from(["cita", "fetch", "--url", "--open", "1207.7214"]).is_ok());
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--cache-only", "--open", "1207.7214"]).is_ok()
        );
        assert!(Cli::try_parse_from(["cita", "fetch", "--url", "--save", "1207.7214"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "open", "1207.7214"]).is_err());
        assert!(Cli::try_parse_from(["cita", "fetch", "--dry-run", "1207.7214"]).is_err());
    }
}
