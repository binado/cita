//! The `bibi` command-line interface.
//!
//! A thin adapter: it parses arguments, constructs concrete providers, renders
//! reports, and chooses an exit code. It never mutates a manifest itself.

mod bootstrap;
mod cli;
mod commands;
mod output;

use clap::Parser;
use cli::{Cli, Command};
use std::process::ExitCode;

/// Exit codes, per the command-line contract.
///
/// A skipped item is not a failure: the requested end state already holds, so
/// `bibi add <locator> && make` proceeds when the reference was already there.
/// Only item failures and operational errors are nonzero. Clap owns exit 2.
const SUCCESS: u8 = 0;
const FAILURE: u8 = 1;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(false) => ExitCode::from(SUCCESS),
        Ok(true) => ExitCode::from(FAILURE),
        Err(error) => {
            output::note(format!("bibi: {error:#}"));
            ExitCode::from(FAILURE)
        }
    }
}

/// Run one command, returning whether any item failed.
async fn run(cli: Cli) -> anyhow::Result<bool> {
    // Completions describe the CLI itself, so they are answered before any
    // platform directory is discovered or any provider is constructed.
    if let Command::Completions(args) = &cli.command {
        output::emit(&cli::completions(args.shell))?;
        return Ok(false);
    }
    let services = bootstrap::services()?;
    let target = cli.target.path.as_deref();
    match cli.command {
        Command::Add(args) => commands::add::run(&services, target, args).await,
        Command::Remove(args) => commands::records::run_remove(target, args),
        Command::Rename(args) => commands::records::run_rename(target, args),
        Command::List(args) => commands::list::run(&services, target, args),
        Command::Show(args) => commands::records::run_show(target, args),
        Command::Sync(args) => commands::sync::run(&services, target, args).await,
        Command::Check(args) => commands::check::run(&services, target, args),
        Command::Fetch(args) => commands::documents::run_fetch(&services, target, args).await,
        Command::Init => commands::init::run(target),
        Command::Completions(_) => unreachable!("answered before services are built"),
    }
}
