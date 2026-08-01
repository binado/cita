//! Command-line argument definitions.

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use std::path::PathBuf;

/// Maintain one project's bibliography aggregate.
#[derive(Debug, Parser)]
#[command(name = "bibi", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Args, Debug)]
pub struct TargetArgs {
    /// Act on this bibi.toml instead of ./bibi.toml
    #[arg(short = 'p', long = "path", global = true, value_name = "FILE")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Resolve references through a provider and store them
    Add(AddArgs),
    /// Import exact local BibTeX from a file or redirected stdin
    Import(ImportArgs),
    /// Delete records
    Remove(RemoveArgs),
    /// List stored records
    List(ListArgs),
    /// Emit exactly one stored record as BibTeX
    Show(ShowArgs),
    /// Unconditionally resolve current state for managed records
    Sync(SyncArgs),
    /// Verify a rendered bibliography
    Check(CheckArgs),
    /// Create an empty bibi.toml
    Init,
    /// Print a shell completion script
    Completions(CompletionsArgs),
}

#[derive(Args, Debug)]
pub struct AddArgs {
    /// arXiv ids, DOIs, provider identities, or k:TEXKEY; omit to read lines from stdin
    #[arg(value_name = "LOCATOR")]
    pub locators: Vec<String>,
    /// Resolve through this provider
    #[arg(long, value_name = "NAME")]
    pub provider: Option<String>,
    /// Replace one unambiguous stable-identity match
    #[arg(long)]
    pub overwrite: bool,
    /// Fully plan without writing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// BibTeX file; omit to read the complete redirected stdin stream
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// Replace one unambiguous stable-identity match
    #[arg(long)]
    pub overwrite: bool,
    /// Fully plan without writing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
    #[arg(skip)]
    pub stdin: Option<String>,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// Locators to remove; omit to read lines from stdin
    #[arg(value_name = "LOCATOR")]
    pub selectors: Vec<String>,
    /// Fully plan without writing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// One locator; omit to read exactly one line from stdin
    #[arg(value_name = "LOCATOR")]
    pub selector: Option<String>,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Listing encoding
    #[arg(long, value_enum, default_value_t = ListFormat::Table)]
    pub format: ListFormat,
    /// Tab-separated fields
    #[arg(long, value_enum, value_delimiter = ',', conflicts_with = "format")]
    pub fields: Vec<Field>,
    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Args, Debug, Default)]
pub struct FilterArgs {
    /// Only records from this provider
    #[arg(long, value_name = "NAME")]
    pub provider: Option<String>,
    /// Author or collaboration substring
    #[arg(long)]
    pub author: Option<String>,
    /// Title substring
    #[arg(long)]
    pub title: Option<String>,
    /// Exact year
    #[arg(long)]
    pub year: Option<i32>,
    /// Only locally imported records
    #[arg(long)]
    pub local: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum ListFormat {
    Table,
    Bibtex,
    Json,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Field {
    Key,
    Title,
    Year,
    Source,
    Doi,
    Arxiv,
    ArxivUrl,
}

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Refresh only this provider
    #[arg(long, value_name = "NAME")]
    pub provider: Option<String>,
    /// Fully plan without writing
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Bibliography file to verify
    #[arg(value_name = "BIBFILE", default_value = "references.bib")]
    pub bibfile: PathBuf,
    /// Describe drift
    #[arg(long)]
    pub diff: bool,
    #[command(flatten)]
    pub filter: FilterArgs,
}

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    #[arg(value_name = "SHELL")]
    pub shell: Shell,
}

pub fn completions(shell: Shell) -> String {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    let mut bytes = Vec::new();
    clap_complete::generate(shell, &mut command, name, &mut bytes);
    String::from_utf8(bytes).expect("completion scripts are UTF-8")
}
