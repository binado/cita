//! Argument definitions.
//!
//! Scope is an argument, not a command level: `-p/--path` is a global flag, so
//! there is no parallel command tree to keep in step and new commands are
//! scope-aware by construction.

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::path::PathBuf;

/// A bibliography CLI that keeps provider-owned references in a manifest.
#[derive(Debug, Parser)]
#[command(name = "bibi", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(subcommand)]
    pub command: Command,
}

/// Which manifest to act on.
#[derive(Args, Debug)]
pub struct TargetArgs {
    /// Act on this manifest instead of `./bibi.toml`
    #[arg(short = 'p', long = "path", global = true, value_name = "FILE")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Resolve references and store them
    Add(AddArgs),
    /// Delete records, emitting what was deleted
    Remove(RemoveArgs),
    /// Change a record's local citation key
    Rename(RenameArgs),
    /// List stored records
    List(ListArgs),
    /// Emit one stored record as BibTeX
    Show(ShowArgs),
    /// Refresh managed records from their providers
    Sync(SyncArgs),
    /// Verify a rendered bibliography against the manifest
    Check(CheckArgs),
    /// Download a record's PDF or source archive
    Fetch(FetchArgs),
    /// Create an empty manifest
    Init,
    /// Print a shell completion script
    Completions(CompletionsArgs),
}

#[derive(Args, Debug)]
pub struct AddArgs {
    /// arXiv ids, DOIs, or `<provider>:<id>` locators
    #[arg(value_name = "LOCATOR")]
    pub locators: Vec<String>,
    /// Read entries from a BibTeX file, or `-` for standard input
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// Store the record under this citation key
    #[arg(long, value_name = "KEY", conflicts_with = "file")]
    pub key: Option<String>,
    /// Resolve only through this provider
    #[arg(long, value_name = "NAME", conflicts_with = "force_local")]
    pub provider: Option<String>,
    /// Replace a matching record instead of skipping it
    #[arg(long)]
    pub overwrite: bool,
    /// Store every entry as supplied, consulting no provider
    #[arg(long = "force-local", requires = "file")]
    pub force_local: bool,
    /// Report what would happen without writing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// Citation keys, DOIs, arXiv ids, or `<provider>:<id>`
    #[arg(value_name = "SELECTOR", required = true)]
    pub selectors: Vec<String>,
    /// Report what would happen without writing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct RenameArgs {
    /// The record to rename
    #[arg(value_name = "SELECTOR")]
    pub selector: String,
    /// Its new citation key
    #[arg(value_name = "KEY")]
    pub key: String,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// How to present the listing
    #[arg(long, value_enum, default_value_t = ListFormat::Table)]
    pub format: ListFormat,
    /// Emit these fields as tab-separated columns
    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        value_name = "FIELD",
        conflicts_with = "format"
    )]
    pub fields: Vec<Field>,
    #[command(flatten)]
    pub filter: FilterArgs,
}

/// The filters that select records, shared by `list` and `check`.
///
/// One set, `--provider` included. The two were split while `export` existed,
/// because there `--provider` named the provider to *sync* first and could not
/// also mean a filter. Nothing names a provider to sync any more.
#[derive(Args, Debug, Default)]
pub struct FilterArgs {
    /// Only records owned by this provider
    #[arg(long, value_name = "NAME")]
    pub provider: Option<String>,
    /// Only records with this substring in an author or collaboration
    #[arg(long, value_name = "TEXT")]
    pub author: Option<String>,
    /// Only records with this substring in the title
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,
    /// Only records from this year
    #[arg(long, value_name = "YEAR")]
    pub year: Option<i32>,
    /// Only records no provider refreshes
    #[arg(long)]
    pub local: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ListFormat {
    /// A human-readable table
    Table,
    /// The stored entries, as a bibliography
    Bibtex,
    /// The schema-1 metadata projection
    Json,
}

/// One column of a `--fields` listing.
///
/// A format says how to encode a listing; a field says what to put in it. They
/// are different questions, which is why one citation key per line is
/// `--fields key` rather than a fourth member of [`ListFormat`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Field {
    /// The local citation key
    Key,
    /// The title
    Title,
    /// The publication year
    Year,
    /// The owning provider
    Provider,
    /// The DOI
    Doi,
    /// The arXiv identifier
    Arxiv,
    /// The public arXiv PDF URL, as `fetch --url` would print it
    ArxivUrl,
}

#[derive(Args, Debug)]
pub struct FetchArgs {
    /// The record to fetch for
    #[arg(value_name = "SELECTOR")]
    pub selector: String,
    /// Fetch the original source archive instead of the PDF
    #[arg(long)]
    pub source: bool,
    /// Print the public URL instead of downloading
    #[arg(long)]
    pub url: bool,
    /// Download to this exact path instead of the default name
    #[arg(short = 'o', long, value_name = "FILE", conflicts_with = "url")]
    pub output: Option<PathBuf>,
    /// Overwrite the destination if it already exists
    #[arg(short = 'f', long, conflicts_with = "url")]
    pub force: bool,
    /// Suppress the download progress bar even on a terminal
    #[arg(long = "no-progress")]
    pub no_progress: bool,
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Refresh only records owned by this provider
    #[arg(long, value_name = "NAME")]
    pub provider: Option<String>,
    /// Refetch every refreshable record, whatever its revision says
    #[arg(long)]
    pub force: bool,
    /// Do the work and report it, but write nothing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// The bibliography to verify
    #[arg(value_name = "BIBFILE", default_value = "references.bib")]
    pub bibfile: PathBuf,
    /// Describe the first difference on drift
    #[arg(long)]
    pub diff: bool,
    /// The same filters `list` selects with, so a filtered view can be verified
    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// The shell to generate for
    #[arg(value_name = "SHELL")]
    pub shell: Shell,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// The record to emit
    #[arg(value_name = "SELECTOR")]
    pub selector: String,
}

/// Write a completion script for `shell` to stdout.
pub fn completions(shell: Shell) -> String {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    let mut rendered = Vec::new();
    clap_complete::generate(shell, &mut command, name, &mut rendered);
    String::from_utf8(rendered).expect("completion scripts are UTF-8")
}
