mod commands;

use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
use std::{env, path::PathBuf};

#[derive(Debug, Parser)]
#[command(name = "cita", version, about = "A personal bibliography library")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize the global cita store and its main shelf
    Init,
    /// Manage global shelves
    Shelf {
        #[command(subcommand)]
        command: ShelfCommand,
    },
    /// Import standalone BibTeX entries from a path or stdin (`-`)
    Import(ImportArgs),
    /// Resolve and add one or more references through INSPIRE
    Add(AddArgs),
    /// Refresh INSPIRE-managed source snapshots
    Sync(ScopeArgs),
    /// Remove references by local key or provider/DOI/arXiv identity
    Remove(RemoveArgs),
    /// List stored references
    List(ListArgs),
    /// Write a URL-enriched BibTeX export
    Export(ExportArgs),
    /// Fetch or resolve a reference's arXiv PDF or source package
    Fetch(FetchArgs),
    /// Generate a shell completion script on stdout
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
enum ShelfCommand {
    /// List global shelves
    #[command(visible_alias = "ls")]
    List,
    /// Create a global shelf
    #[command(visible_alias = "create")]
    New {
        /// Stable shelf name
        name: String,
    },
}

#[derive(Debug, Args)]
struct ShelfArg {
    /// Use this global shelf instead of `main`
    #[arg(short = 's', long, value_name = "NAME")]
    shelf: Option<String>,
}

#[derive(Debug, Args)]
struct ScopeArgs {
    /// Use this global shelf instead of `main`
    #[arg(short = 's', long, value_name = "NAME", conflicts_with = "all_shelves")]
    shelf: Option<String>,
    /// Run in every global shelf, in name order
    #[arg(long)]
    all_shelves: bool,
}

#[derive(Debug, Args)]
struct ImportArgs {
    /// Replace colliding existing entries instead of skipping them
    #[arg(long)]
    overwrite: bool,
    #[command(flatten)]
    scope: ShelfArg,
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
    #[command(flatten)]
    scope: ShelfArg,
    /// INSPIRE locator: arXiv ID, `arxiv:`, `doi:`, `inspire:`, or canonical URL
    #[arg(required = true)]
    locators: Vec<String>,
}

#[derive(Debug, Args)]
struct RemoveArgs {
    #[command(flatten)]
    scope: ShelfArg,
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
    #[command(flatten)]
    scope: ShelfArg,
}

#[derive(Debug, Args)]
struct ExportArgs {
    #[command(flatten)]
    scope: ScopeArgs,
    /// Output file, or output directory with --all-shelves
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
    /// Save an unmatched INSPIRE locator to the selected shelf before fetching
    #[arg(long)]
    save: bool,
    #[command(flatten)]
    scope: ShelfArg,
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
    match run().await {
        Ok(RunOutcome::Success) => {}
        Ok(RunOutcome::ReportedFailure) => std::process::exit(1),
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::exit(1);
        }
    }
}

enum RunOutcome {
    Success,
    ReportedFailure,
}

async fn run() -> Result<RunOutcome> {
    let cli = Cli::parse();
    let cwd = env::current_dir().context("could not determine current directory")?;
    let mut batch_failed = false;
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
        }
        Some(Command::Init) => commands::init_global()?,
        Some(Command::Shelf { command }) => match command {
            ShelfCommand::List => commands::list_shelves()?,
            ShelfCommand::New { name } => commands::new_shelf(&name)?,
        },
        Some(Command::Import(args)) => {
            let target = commands::resolve_target(args.scope.shelf.as_deref())?;
            commands::import(&target, &cwd, &args.path, args.overwrite)?
        }
        Some(Command::Add(AddArgs {
            key,
            overwrite,
            scope,
            locators,
        })) => {
            let target = commands::resolve_target(scope.shelf.as_deref())?;
            commands::add(&target, key.as_deref(), &locators, overwrite).await?
        }
        Some(Command::Sync(scope)) => {
            if scope.all_shelves {
                batch_failed = commands::batch_sync().await?;
            } else {
                let target = commands::resolve_target(scope.shelf.as_deref())?;
                commands::sync(&target).await?;
            }
        }
        Some(Command::Remove(args)) => {
            let target = commands::resolve_target(args.scope.shelf.as_deref())?;
            commands::remove(&target, &args.selectors)?
        }
        Some(Command::List(ListArgs {
            sort_by,
            order,
            no_wrap_title,
            scope,
        })) => {
            let target = commands::resolve_target(scope.shelf.as_deref())?;
            commands::list(&target, sort_by, order, !no_wrap_title)?
        }
        Some(Command::Export(args)) => {
            if args.scope.all_shelves {
                batch_failed = commands::batch_export(&cwd, args.output.as_deref())?;
            } else {
                let target = commands::resolve_target(args.scope.shelf.as_deref())?;
                commands::export(&target, &cwd, args.output.as_deref())?;
            }
        }
        Some(Command::Fetch(args)) => {
            let target = commands::resolve_target(args.scope.shelf.as_deref())?;
            let (selector, options) = args.into_options();
            commands::fetch(&target, &selector, options).await?
        }
        Some(Command::Completions { shell }) => {
            let mut command = Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
        }
    }
    if batch_failed {
        Ok(RunOutcome::ReportedFailure)
    } else {
        Ok(RunOutcome::Success)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_command_matrix_is_accepted() {
        for args in [
            vec!["cita", "init"],
            vec!["cita", "shelf", "list"],
            vec!["cita", "shelf", "ls"],
            vec!["cita", "shelf", "new", "paper"],
            vec!["cita", "shelf", "create", "paper"],
            vec!["cita", "add", "-s", "paper", "1207.7214"],
            vec!["cita", "import", "-s", "paper", "-"],
            vec!["cita", "remove", "-s", "paper", "Key"],
            vec!["cita", "list", "-s", "paper"],
            vec!["cita", "sync", "--all-shelves"],
            vec!["cita", "export", "-s", "paper", "references.bib"],
            vec!["cita", "export", "--all-shelves", "exports"],
            vec!["cita", "fetch", "-s", "paper", "Key"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn removed_commands_and_flags_are_rejected() {
        for args in [
            vec!["cita", "init", "--path", "somewhere"],
            vec!["cita", "library", "list"],
            vec!["cita", "generate"],
            vec!["cita", "commit"],
            vec!["cita", "export", "--output", "x.bib"],
            vec!["cita", "sync", "-s", "paper", "--all-shelves"],
            vec!["cita", "export", "-s", "paper", "--all-shelves"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn key_and_document_conflicts_are_enforced() {
        assert!(Cli::try_parse_from(["cita", "add", "--key", "X", "1207.7214"]).is_ok());
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--force", "--cache-only", "1207.7214"]).is_err()
        );
        assert!(Cli::try_parse_from(["cita", "fetch", "--source", "--url", "1207.7214"]).is_err());
    }
}
