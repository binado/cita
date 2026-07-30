//! The `bibi` command-line interface.
//!
//! A thin adapter: it parses arguments, constructs concrete providers, renders
//! reports, and chooses an exit code. It never mutates a manifest itself.

mod bootstrap;
mod cli;
mod commands;
mod input;
mod output;

use clap::{CommandFactory, Parser, error::ErrorKind};
use cli::{Cli, Command};
use std::{io::IsTerminal, process::ExitCode};

/// Exit codes, per the command-line contract.
///
/// A skipped item is not a failure: the requested end state already holds, so
/// `bibi add <locator> && make` proceeds when the reference was already there.
/// Only item failures and operational errors are nonzero. Clap owns exit 2.
const SUCCESS: u8 = 0;
const FAILURE: u8 = 1;

#[tokio::main]
async fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let stdin = std::io::stdin();
    if let Err(error) = input::resolve(&mut cli.command, stdin.is_terminal(), stdin.lock()) {
        match error {
            input::Error::Usage(message) => {
                let error = usage_error(&cli.command, message);
                let _ = error.print();
                return ExitCode::from(2);
            }
            input::Error::Read(error) => {
                output::note(format!("bibi: reading standard input: {error}"));
                return ExitCode::from(FAILURE);
            }
        }
    }
    if empty_batch(&cli.command) {
        return ExitCode::from(SUCCESS);
    }
    match run(cli).await {
        Ok(false) => ExitCode::from(SUCCESS),
        Ok(true) => ExitCode::from(FAILURE),
        Err(error) => {
            output::note(format!("bibi: {error:#}"));
            ExitCode::from(FAILURE)
        }
    }
}

fn usage_error(command: &Command, message: String) -> clap::Error {
    let name = match command {
        Command::Add(_) => "add",
        Command::Fetch(_) => "fetch",
        Command::Remove(_) => "remove",
        Command::Show(_) => "show",
        _ => unreachable!("only commands with optional stdin produce usage errors here"),
    };
    let mut root = Cli::command();
    let mut subcommand = root
        .find_subcommand_mut(name)
        .expect("the parsed subcommand exists")
        .clone()
        .bin_name(format!("bibi {name}"));
    subcommand.error(ErrorKind::MissingRequiredArgument, message)
}

fn empty_batch(command: &Command) -> bool {
    match command {
        Command::Add(args) => args.file.is_none() && args.locators.is_empty(),
        Command::Fetch(args) => args.selectors.is_empty(),
        Command::Remove(args) => args.selectors.is_empty(),
        _ => false,
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
        Command::List(args) => commands::list::run(target, args),
        Command::Show(args) => commands::records::run_show(target, args),
        Command::Sync(args) => commands::sync::run(&services, target, args).await,
        Command::Check(args) => commands::check::run(target, args),
        Command::Fetch(args) => commands::documents::run_fetch(&services, target, args).await,
        Command::Init => commands::init::run(target),
        Command::Completions(_) => unreachable!("answered before services are built"),
    }
}
