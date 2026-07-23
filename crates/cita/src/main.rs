mod commands;
mod git;

use anyhow::{Context, Result};
use clap::{Args, CommandFactory, Parser, Subcommand};
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
    Init(InitArgs),
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
    /// Regenerate a missing or edited references.bib
    Generate,
    /// Write a derived BibTeX export with arXiv PDF URLs, for tools like Zotero
    Export(ExportArgs),
    /// Fetch or resolve a reference's arXiv PDF or source package
    Fetch(FetchArgs),
    /// Commit the managed files; refuses to run if either managed file is already staged
    Commit,
    /// Manage a multi-project cita library
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
    /// Generate a shell completion script on stdout
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Initialize in this existing directory instead of the current directory
    #[arg(long)]
    path: Option<PathBuf>,
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
    /// Write the export here instead of `<project>.bib`; relative paths use the caller's directory
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

#[derive(Debug, Subcommand)]
enum LibraryCommand {
    /// Initialize cita-library.toml in an existing directory
    Init(InitArgs),
    /// List registered shelves
    Shelves,
    /// Run a command for one registered shelf
    Shelf {
        /// Stable registered shelf name
        name: String,
        #[command(subcommand)]
        command: ShelfCommand,
    },
    /// Regenerate references.bib in every shelf
    Generate,
    /// Write a `<shelf>.bib` export in every shelf
    Export,
    /// Refresh INSPIRE snapshots in every shelf
    Sync,
}

#[derive(Debug, Subcommand)]
enum ShelfCommand {
    /// Initialize and register a shelf
    Init {
        /// Library-root-relative shelf directory; defaults to the shelf name
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Import standalone BibTeX entries; relative paths use the caller's directory
    Import(ImportArgs),
    /// Resolve and add references through INSPIRE
    Add(AddArgs),
    /// Refresh INSPIRE-managed snapshots
    Sync,
    /// Remove references by selector
    Remove(RemoveArgs),
    /// List stored references
    List(ListArgs),
    /// Regenerate references.bib
    Generate,
    /// Write a derived BibTeX export named for this shelf
    Export(ExportArgs),
    /// Fetch or resolve an arXiv document
    Fetch(FetchArgs),
    /// Commit this shelf's managed files; refuses to run if either managed file is already staged
    Commit,
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
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
        }
        Some(Command::Init(args)) => commands::init(&cwd, args.path.as_deref())?,
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
        Some(Command::Generate) => commands::generate(&cwd)?,
        Some(Command::Export(args)) => commands::export(&cwd, &cwd, args.output.as_deref())?,
        Some(Command::Fetch(args)) => {
            let (selector, options) = args.into_options();
            commands::fetch(&cwd, &selector, options).await?
        }
        Some(Command::Commit) => git::commit(&commands::find_manifest(&cwd)?)?,
        Some(Command::Library { command }) => {
            let failed = match command {
                LibraryCommand::Init(args) => {
                    commands::init_library(&cwd, args.path.as_deref())?;
                    Ok(false)
                }
                LibraryCommand::Shelves => {
                    commands::list_shelves(&cwd)?;
                    Ok(false)
                }
                LibraryCommand::Shelf { name, command } => {
                    let action = match command {
                        ShelfCommand::Init { path } => {
                            commands::init_shelf(&cwd, &name, path.as_deref())?;
                            None
                        }
                        ShelfCommand::Import(args) => Some(commands::ShelfAction::Import(args)),
                        ShelfCommand::Add(args) => Some(commands::ShelfAction::Add(args)),
                        ShelfCommand::Sync => Some(commands::ShelfAction::Sync),
                        ShelfCommand::Remove(args) => Some(commands::ShelfAction::Remove(args)),
                        ShelfCommand::List(args) => Some(commands::ShelfAction::List(args)),
                        ShelfCommand::Generate => Some(commands::ShelfAction::Generate),
                        ShelfCommand::Export(args) => Some(commands::ShelfAction::Export(args)),
                        ShelfCommand::Fetch(args) => Some(commands::ShelfAction::Fetch(args)),
                        ShelfCommand::Commit => Some(commands::ShelfAction::Commit),
                    };
                    if let Some(action) = action {
                        commands::run_shelf_command(&cwd, &name, action).await?;
                    }
                    Ok(false)
                }
                LibraryCommand::Generate => commands::library_generate(&cwd),
                LibraryCommand::Export => commands::library_export(&cwd),
                LibraryCommand::Sync => commands::library_sync(&cwd).await,
            }?;
            if failed {
                return Ok(RunOutcome::ReportedFailure);
            }
        }
        Some(Command::Completions { shell }) => {
            let mut command = Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
        }
    }
    Ok(RunOutcome::Success)
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
        assert!(Cli::try_parse_from(["cita", "fetch", "--source", "--url", "1207.7214"]).is_err());
        assert!(
            Cli::try_parse_from([
                "cita",
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
            Cli::try_parse_from(["cita", "fetch", "--source", "--cache-only", "1207.7214"]).is_ok()
        );
        assert!(Cli::try_parse_from(["cita", "open", "1207.7214"]).is_err());
        assert!(Cli::try_parse_from(["cita", "fetch", "--dry-run", "1207.7214"]).is_err());
    }
    #[test]
    fn completions_accepts_known_shells_only() {
        assert!(Cli::try_parse_from(["cita", "completions", "zsh"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "completions", "nonsense"]).is_err());
    }

    #[test]
    fn library_command_tree_routes_supported_shelf_operations() {
        for args in [
            vec!["cita", "library", "init"],
            vec!["cita", "library", "shelves"],
            vec!["cita", "library", "shelf", "paper", "init"],
            vec!["cita", "library", "shelf", "paper", "add", "1207.7214"],
            vec!["cita", "library", "shelf", "paper", "import", "-"],
            vec!["cita", "library", "shelf", "paper", "remove", "Key"],
            vec!["cita", "library", "shelf", "paper", "list"],
            vec!["cita", "library", "shelf", "paper", "generate"],
            vec!["cita", "library", "shelf", "paper", "sync"],
            vec!["cita", "library", "shelf", "paper", "fetch", "Key"],
            vec!["cita", "library", "shelf", "paper", "commit"],
            vec!["cita", "library", "shelf", "paper", "export"],
            vec!["cita", "library", "shelf", "paper", "export", "-o", "x.bib"],
            vec!["cita", "library", "generate"],
            vec!["cita", "library", "export"],
            vec!["cita", "library", "sync"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        assert!(Cli::try_parse_from(["cita", "library", "commit"]).is_err());
    }

    #[test]
    fn export_takes_an_optional_output_flag_but_never_a_path_argument() {
        assert!(Cli::try_parse_from(["cita", "export"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "export", "-o", "x.bib"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "export", "--output", "x.bib"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "export", "x.bib"]).is_err());
        // One --output cannot name a file for each shelf, so the batch form
        // deliberately takes no path.
        assert!(Cli::try_parse_from(["cita", "library", "export", "-o", "x.bib"]).is_err());
    }
}
