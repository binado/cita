//! Argument definitions.
//!
//! Scope is an argument, not a command level: `-p/--path` and `-g/--global` are
//! global flags, so there is no parallel command tree to keep in step and new
//! commands are scope-aware by construction.

use clap::{Args, Parser, Subcommand, ValueEnum};
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
    #[arg(
        short = 'p',
        long = "path",
        global = true,
        value_name = "FILE",
        conflicts_with = "global"
    )]
    pub path: Option<PathBuf>,
    /// Act on the user-level manifest
    #[arg(short = 'g', long = "global", global = true)]
    pub global: bool,
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
    /// Create an empty manifest
    Init,
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
    #[command(flatten)]
    pub filter: FilterArgs,
}

/// The listing filters, shared with the commands that render a bibliography.
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
    /// One citation key per line
    Keys,
    /// The stored entries, as a bibliography
    Bibtex,
    /// The schema-1 metadata projection
    Json,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// The record to emit
    #[arg(value_name = "SELECTOR")]
    pub selector: String,
}
