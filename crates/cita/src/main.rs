mod commands;

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
    Sync(ScopeArgs),
    /// Remove references by local key or provider/DOI/arXiv identity
    Remove(RemoveArgs),
    /// List stored references
    List(ListArgs),
    /// Edit the whole manifest in $VISUAL, $EDITOR, or vi
    Edit(ShelfArg),
    /// Regenerate a missing or edited references.bib
    Generate(ScopeArgs),
    /// Write a derived BibTeX export with arXiv PDF URLs, for tools like Zotero
    Export(ExportArgs),
    /// Fetch or resolve a reference's arXiv PDF or source package
    Fetch(FetchArgs),
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

/// Selects one registered shelf instead of the project discovered from the
/// caller's directory.
///
/// Flattened into each command rather than declared once as a global argument,
/// so it never appears in the help for commands that cannot honor it.
#[derive(Debug, Args)]
struct ShelfArg {
    /// Run in this registered shelf instead of the discovered project
    #[arg(short = 's', long, value_name = "NAME")]
    shelf: Option<String>,
}

/// `ShelfArg` plus the every-shelf batch form, for the commands that are safe to
/// repeat across a whole library: they are idempotent and derive their result
/// from each shelf's own manifest.
#[derive(Debug, Args)]
struct ScopeArgs {
    /// Run in this registered shelf instead of the discovered project
    #[arg(short = 's', long, value_name = "NAME", conflicts_with = "all_shelves")]
    shelf: Option<String>,
    /// Run in every registered shelf, in name order
    #[arg(long)]
    all_shelves: bool,
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
    /// Show only references carrying this tag; repeat to require every tag
    #[arg(long = "tag", value_name = "TAG")]
    tags: Vec<String>,
    #[command(flatten)]
    scope: ShelfArg,
}

#[derive(Debug, Args)]
struct ExportArgs {
    /// Write the export here instead of the default `<name>.bib`; relative paths use the caller's directory
    // One --output cannot name a file for each shelf, so it is refused with the
    // batch form rather than silently applying to only the last shelf.
    #[arg(short = 'o', long, conflicts_with = "all_shelves")]
    output: Option<PathBuf>,
    #[command(flatten)]
    scope: ScopeArgs,
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

/// Shelf lifecycle only. Operations *within* a shelf are reached with `--shelf`
/// on the ordinary command, so the two command lists cannot drift apart.
#[derive(Debug, Subcommand)]
enum LibraryCommand {
    /// Initialize cita-library.toml in an existing directory
    Init(InitArgs),
    /// List registered shelves
    #[command(visible_alias = "ls")]
    List,
    /// Initialize and register a shelf
    #[command(visible_alias = "create")]
    New {
        /// Stable registered shelf name
        name: String,
        /// Library-root-relative shelf directory; defaults to the shelf name
        #[arg(long)]
        path: Option<PathBuf>,
    },
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
    // Set only by the batch forms, which report their own per-shelf failures and
    // continue rather than returning an error.
    let mut batch_failed = false;
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
        }
        Some(Command::Init(args)) => commands::init(&cwd, args.path.as_deref())?,
        Some(Command::Import(args)) => {
            let target = commands::resolve_target(&cwd, args.scope.shelf.as_deref())?;
            commands::import(&target.directory, &args.path, args.overwrite)?
        }
        Some(Command::Add(AddArgs {
            key,
            overwrite,
            scope,
            locators,
        })) => {
            let target = commands::resolve_target(&cwd, scope.shelf.as_deref())?;
            commands::add(&target.directory, key.as_deref(), &locators, overwrite).await?
        }
        Some(Command::Sync(scope)) => {
            if scope.all_shelves {
                batch_failed = commands::library_sync(&cwd).await?;
            } else {
                let target = commands::resolve_target(&cwd, scope.shelf.as_deref())?;
                commands::sync(&target.directory).await?;
            }
        }
        Some(Command::Remove(args)) => {
            let target = commands::resolve_target(&cwd, args.scope.shelf.as_deref())?;
            commands::remove(&target.directory, &args.selectors)?
        }
        Some(Command::List(ListArgs {
            sort_by,
            order,
            no_wrap_title,
            tags,
            scope,
        })) => {
            let target = commands::resolve_target(&cwd, scope.shelf.as_deref())?;
            commands::list(&target.directory, sort_by, order, !no_wrap_title, &tags)?
        }
        Some(Command::Edit(scope)) => {
            let target = commands::resolve_target(&cwd, scope.shelf.as_deref())?;
            commands::edit(&target.directory)?
        }
        Some(Command::Generate(scope)) => {
            if scope.all_shelves {
                batch_failed = commands::library_generate(&cwd).await?;
            } else {
                let target = commands::resolve_target(&cwd, scope.shelf.as_deref())?;
                commands::generate(&target.directory)?;
            }
        }
        Some(Command::Export(args)) => {
            if args.scope.all_shelves {
                batch_failed = commands::library_export(&cwd).await?;
            } else {
                let target = commands::resolve_target(&cwd, args.scope.shelf.as_deref())?;
                // A shelf export is named for its stable registered name, which
                // can differ from the directory the shelf is registered at.
                let default = target.export_path();
                let output = args.output.as_deref().or(default.as_deref());
                commands::export(&target.directory, &cwd, output)?;
            }
        }
        Some(Command::Fetch(args)) => {
            let target = commands::resolve_target(&cwd, args.scope.shelf.as_deref())?;
            let (selector, options) = args.into_options();
            commands::fetch(&target.directory, &selector, options).await?
        }
        Some(Command::Library { command }) => match command {
            LibraryCommand::Init(args) => commands::init_library(&cwd, args.path.as_deref())?,
            LibraryCommand::List => commands::list_shelves(&cwd)?,
            LibraryCommand::New { name, path } => {
                commands::init_shelf(&cwd, &name, path.as_deref())?
            }
        },
        Some(Command::Completions { shell }) => {
            let mut command = Cli::command();
            let name = command.get_name().to_string();
            clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
        }
    }
    if batch_failed {
        return Ok(RunOutcome::ReportedFailure);
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
    fn library_manages_shelf_lifecycle_only() {
        for args in [
            vec!["cita", "library", "init"],
            vec!["cita", "library", "init", "--path", "shelves"],
            vec!["cita", "library", "list"],
            vec!["cita", "library", "ls"],
            vec!["cita", "library", "new", "paper"],
            vec!["cita", "library", "create", "paper"],
            vec!["cita", "library", "new", "paper", "--path", "papers/one"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        // Operations inside a shelf are reached with --shelf, not through the
        // library tree.
        for args in [
            vec!["cita", "library", "shelves"],
            vec!["cita", "library", "shelf", "paper", "list"],
            vec!["cita", "library", "add", "paper"],
            vec!["cita", "library", "generate"],
            vec!["cita", "library", "export"],
            vec!["cita", "library", "sync"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn every_project_command_accepts_a_shelf() {
        for args in [
            vec!["cita", "add", "-s", "paper", "1207.7214"],
            vec!["cita", "add", "1207.7214", "--shelf", "paper"],
            vec!["cita", "import", "-s", "paper", "-"],
            vec!["cita", "remove", "-s", "paper", "Key"],
            vec!["cita", "list", "-s", "paper"],
            vec!["cita", "edit", "-s", "paper"],
            vec!["cita", "generate", "-s", "paper"],
            vec!["cita", "export", "-s", "paper"],
            vec!["cita", "export", "-s", "paper", "-o", "x.bib"],
            vec!["cita", "sync", "-s", "paper"],
            vec!["cita", "fetch", "-s", "paper", "Key"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn only_idempotent_derived_commands_run_across_every_shelf() {
        for args in [
            vec!["cita", "generate", "--all-shelves"],
            vec!["cita", "export", "--all-shelves"],
            vec!["cita", "sync", "--all-shelves"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_ok(), "{args:?}");
        }
        for args in [
            vec!["cita", "add", "--all-shelves", "1207.7214"],
            vec!["cita", "import", "--all-shelves", "-"],
            vec!["cita", "remove", "--all-shelves", "Key"],
            vec!["cita", "list", "--all-shelves"],
            vec!["cita", "edit", "--all-shelves"],
            vec!["cita", "fetch", "--all-shelves", "Key"],
            // One shelf or every shelf, never both.
            vec!["cita", "sync", "-s", "paper", "--all-shelves"],
            vec!["cita", "generate", "-s", "paper", "--all-shelves"],
            vec!["cita", "export", "-s", "paper", "--all-shelves"],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_err(), "{args:?}");
        }
    }

    #[test]
    fn list_accepts_repeatable_tag_filters() {
        for args in [
            vec!["cita", "list", "--tag", "higgs"],
            vec!["cita", "list", "--tag", "higgs", "--tag", "atlas"],
            vec![
                "cita",
                "list",
                "--tag",
                "higgs",
                "--sort-by",
                "year",
                "-s",
                "paper",
            ],
        ] {
            assert!(Cli::try_parse_from(args.clone()).is_ok(), "{args:?}");
        }
        // A tag is a value, never a bare positional or a comma-joined list.
        assert!(Cli::try_parse_from(["cita", "list", "--tag"]).is_err());
        assert!(Cli::try_parse_from(["cita", "list", "higgs"]).is_err());
        let cli = Cli::try_parse_from(["cita", "list", "--tag", "a", "--tag", "b"]).unwrap();
        let Some(Command::List(args)) = cli.command else {
            unreachable!()
        };
        assert_eq!(args.tags, ["a", "b"]);
    }

    #[test]
    fn export_takes_an_optional_output_flag_but_never_a_path_argument() {
        assert!(Cli::try_parse_from(["cita", "export"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "export", "-o", "x.bib"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "export", "--output", "x.bib"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "export", "x.bib"]).is_err());
        // One --output cannot name a file for each shelf, so the batch form
        // deliberately takes no path.
        assert!(Cli::try_parse_from(["cita", "export", "--all-shelves", "-o", "x.bib"]).is_err());
    }
}
