mod commands;
mod git;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use std::{env, path::PathBuf};

#[derive(Debug, Parser)]
#[command(name = "cita", version, about = "A Git-friendly bibliography database")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize cita.toml and its generated references.bib
    Init {
        /// Initialize in this existing directory instead of the current directory
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Import standalone BibTeX entries from a path or stdin (`-`)
    Import {
        /// Replace colliding existing entries instead of skipping them
        #[arg(long)]
        overwrite: bool,
        /// BibTeX file to import, or `-` to read from standard input
        path: String,
    },
    /// Resolve and add one or more references through INSPIRE
    Add {
        /// Keep this local citation key (one locator only)
        #[arg(long)]
        key: Option<String>,
        /// Replace colliding existing entries instead of skipping them
        #[arg(long)]
        overwrite: bool,
        /// INSPIRE locator: arXiv ID, `arxiv:`, `doi:`, or `inspire:`
        #[arg(required = true)]
        locators: Vec<String>,
    },
    /// Refresh every INSPIRE-managed source snapshot by stable record id
    Sync,
    /// Remove references by local key or provider/DOI/arXiv identity
    Remove {
        /// Local key, provider ID, DOI, or arXiv ID to remove
        #[arg(required = true)]
        selectors: Vec<String>,
    },
    /// List stored references
    List {
        /// Field used to sort the displayed references
        #[arg(long, value_enum, default_value_t = SortBy::Key)]
        sort_by: SortBy,
        /// Sort direction; references without a year remain last
        #[arg(long, value_enum, default_value_t = Order::Asc)]
        order: Order,
        /// Do not wrap long titles to the terminal width
        #[arg(long)]
        no_wrap_title: bool,
    },
    /// Regenerate a missing or edited references.bib
    Generate,
    /// Fetch or resolve a reference's arXiv PDF
    Fetch {
        /// Download again even when a valid PDF is already cached
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
        /// Save an unmatched INSPIRE locator to the manifest before fetching
        #[arg(long)]
        save: bool,
        /// Local key, provider ID, DOI, arXiv ID, or unmatched INSPIRE locator
        selector: String,
    },
    /// Commit the managed files; refuses to run if either managed file is already staged
    Commit,
    /// Generate a shell completion script on stdout
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
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
        Some(Command::Init { path }) => commands::init(&cwd, path.as_deref())?,
        Some(Command::Import { overwrite, path }) => commands::import(&cwd, &path, overwrite)?,
        Some(Command::Add {
            key,
            overwrite,
            locators,
        }) => commands::add(&cwd, key.as_deref(), &locators, overwrite).await?,
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
        Some(Command::Completions { shell }) => {
            let mut command = Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
        }
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
    #[test]
    fn completions_accepts_known_shells_only() {
        assert!(Cli::try_parse_from(["cita", "completions", "zsh"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "completions", "nonsense"]).is_err());
    }
}
