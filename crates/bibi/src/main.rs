mod commands;

use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
use std::{env, path::PathBuf};

#[derive(Debug, Parser)]
#[command(name = "bibi", version, about = "A Git-friendly bibliography database")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Import standalone BibTeX entries from a path or stdin (`-`)
    Import(ImportArgs),
    /// Resolve and add one or more references through INSPIRE
    Add(AddArgs),
    /// Refresh every INSPIRE-managed source snapshot by stable record id
    Sync,
    /// Remove references by local key or provider/DOI/arXiv identity
    Remove(RemoveArgs),
    /// List stored references
    List(ListArgs),
    /// Write a derived BibTeX export with arXiv PDF URLs, for tools like Zotero
    Export(ExportArgs),
    /// Fetch or resolve a reference's arXiv PDF or source package
    Fetch(FetchArgs),
    /// Generate a shell completion script on stdout
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// Replace colliding existing entries instead of skipping them
    #[arg(long)]
    overwrite: bool,
    /// BibTeX file to import, or `-` to read from standard input
    path: String,
}

#[derive(Debug, Args)]
struct AddArgs {
    /// Keep this local citation key (one locator only)
    #[arg(long)]
    key: Option<String>,
    /// Replace colliding existing entries instead of skipping them
    #[arg(long)]
    overwrite: bool,
    /// INSPIRE locator: arXiv ID, `arxiv:`, `doi:`, `inspire:`, or canonical URL
    #[arg(required = true)]
    locators: Vec<String>,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    /// Local key, provider ID, DOI, arXiv ID, or canonical URL to remove
    #[arg(required = true)]
    selectors: Vec<String>,
}

#[derive(Debug, Args)]
struct ListArgs {
    /// Field used to sort the displayed references
    #[arg(long, value_enum, default_value_t = SortBy::Key)]
    sort_by: SortBy,
    /// Sort direction; references without a year remain last
    #[arg(long, value_enum, default_value_t = Order::Asc)]
    order: Order,
    /// Do not wrap long titles to the terminal width
    #[arg(long)]
    no_wrap_title: bool,
}

#[derive(Debug, Args)]
struct ExportArgs {
    /// Write the export here instead of the default `<name>.bib`; relative paths use the caller's directory
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct FetchArgs {
    /// Download again even when a valid artifact is already cached
    #[arg(long, conflicts_with_all = ["cache_only", "url"])]
    force: bool,
    /// Require an existing cached artifact without downloading
    #[arg(long, conflicts_with = "url")]
    cache_only: bool,
    /// Return the arXiv PDF URL without downloading
    #[arg(short = 'u', long)]
    url: bool,
    /// Download and extract the latest arXiv TeX source package
    #[arg(long, conflicts_with = "url")]
    source: bool,
    /// Open the returned path or URL with the system default application
    #[arg(long)]
    open: bool,
    /// Save an unmatched INSPIRE locator to the manifest before fetching
    #[arg(long)]
    save: bool,
    /// Local key, provider ID, DOI, arXiv ID, canonical URL, or unmatched INSPIRE locator
    selector: String,
}

impl FetchArgs {
    fn into_options(self) -> (String, commands::FetchOptions) {
        (
            self.selector,
            commands::FetchOptions {
                force: self.force,
                cache_only: self.cache_only,
                return_url: self.url,
                source: self.source,
                open: self.open,
                save: self.save,
            },
        )
    }
}

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub(crate) enum SortBy {
    #[default]
    Key,
    Title,
    Author,
    Year,
}
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub(crate) enum Order {
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
        Some(Command::Import(args)) => commands::import(&cwd, &args.path, args.overwrite)?,
        Some(Command::Add(AddArgs {
            key,
            overwrite,
            locators,
        })) => commands::add(&cwd, key.as_deref(), &locators, overwrite).await?,
        Some(Command::Sync) => commands::sync(&cwd).await?,
        Some(Command::Remove(args)) => commands::remove(&cwd, &args.selectors)?,
        Some(Command::List(ListArgs {
            sort_by,
            order,
            no_wrap_title,
        })) => commands::list(&cwd, sort_by, order, !no_wrap_title)?,
        Some(Command::Export(args)) => commands::export(&cwd, &cwd, args.output.as_deref())?,
        Some(Command::Fetch(args)) => {
            let (selector, options) = args.into_options();
            commands::fetch(&cwd, &selector, options).await?
        }
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
        assert!(Cli::try_parse_from(["bibi", "add", "--key", "X", "1207.7214"]).is_ok());
    }
    #[test]
    fn document_conflicts_are_enforced() {
        assert!(
            Cli::try_parse_from(["bibi", "fetch", "--force", "--cache-only", "1207.7214"]).is_err()
        );
        assert!(Cli::try_parse_from(["bibi", "fetch", "--url", "--force", "1207.7214"]).is_err());
        assert!(
            Cli::try_parse_from(["bibi", "fetch", "--url", "--cache-only", "1207.7214"]).is_err()
        );
        assert!(Cli::try_parse_from(["bibi", "fetch", "--url", "--open", "1207.7214"]).is_ok());
        assert!(
            Cli::try_parse_from(["bibi", "fetch", "--cache-only", "--open", "1207.7214"]).is_ok()
        );
        assert!(Cli::try_parse_from(["bibi", "fetch", "--url", "--save", "1207.7214"]).is_ok());
        assert!(Cli::try_parse_from(["bibi", "fetch", "--source", "--url", "1207.7214"]).is_err());
        assert!(
            Cli::try_parse_from([
                "bibi",
                "fetch",
                "--source",
                "--force",
                "--open",
                "--save",
                "1207.7214"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from(["bibi", "fetch", "--source", "--cache-only", "1207.7214"]).is_ok()
        );
        assert!(Cli::try_parse_from(["bibi", "open", "1207.7214"]).is_err());
        assert!(Cli::try_parse_from(["bibi", "fetch", "--dry-run", "1207.7214"]).is_err());
    }
    #[test]
    fn completions_accepts_known_shells_only() {
        assert!(Cli::try_parse_from(["bibi", "completions", "zsh"]).is_ok());
        assert!(Cli::try_parse_from(["bibi", "completions", "nonsense"]).is_err());
    }

    /// Scope is not a concept: no command takes a shelf or a batch form, and
    /// the library tree is gone.
    #[test]
    fn no_command_accepts_a_scope() {
        for args in [
            vec!["bibi", "library", "list"],
            vec!["bibi", "add", "-s", "paper", "1207.7214"],
            vec!["bibi", "list", "--shelf", "paper"],
            vec!["bibi", "sync", "--all-shelves"],
            vec!["bibi", "export", "--all-shelves"],
            vec!["bibi", "init"],
            vec!["bibi", "generate"],
            vec!["bibi", "commit"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn export_takes_an_optional_output_flag_but_never_a_path_argument() {
        assert!(Cli::try_parse_from(["bibi", "export"]).is_ok());
        assert!(Cli::try_parse_from(["bibi", "export", "-o", "x.bib"]).is_ok());
        assert!(Cli::try_parse_from(["bibi", "export", "--output", "x.bib"]).is_ok());
        assert!(Cli::try_parse_from(["bibi", "export", "x.bib"]).is_err());
    }
}
