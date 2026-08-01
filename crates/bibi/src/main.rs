//! The bibi command-line adapter.

mod bootstrap;
mod cli;
mod commands;
mod input;
mod output;

use clap::{CommandFactory, Parser, error::ErrorKind};
use cli::{Cli, Command};
use std::{io::IsTerminal, process::ExitCode};

#[tokio::main]
async fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let stdin = std::io::stdin();
    if let Err(error) = input::resolve(&mut cli.command, stdin.is_terminal(), stdin.lock()) {
        match error {
            input::Error::Usage(message) => {
                let _ = usage_error(&cli.command, message).print();
                return ExitCode::from(2);
            }
            input::Error::Read(error) => output::note(format!("bibi: reading stdin: {error}")),
        }
        return ExitCode::from(1);
    }
    if matches!(&cli.command, Command::Add(args) if args.locators.is_empty())
        || matches!(&cli.command, Command::Remove(args) if args.selectors.is_empty())
    {
        return ExitCode::SUCCESS;
    }
    match run(cli).await {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(error) => {
            output::note(format!("bibi: {error:#}"));
            ExitCode::from(1)
        }
    }
}

fn usage_error(command: &Command, message: String) -> clap::Error {
    let name = match command {
        Command::Add(_) => "add",
        Command::Import(_) => "import",
        Command::Remove(_) => "remove",
        Command::Show(_) => "show",
        _ => unreachable!(),
    };
    Cli::command()
        .find_subcommand_mut(name)
        .expect("subcommand")
        .clone()
        .bin_name(format!("bibi {name}"))
        .error(ErrorKind::MissingRequiredArgument, message)
}

async fn run(cli: Cli) -> anyhow::Result<bool> {
    if let Command::Completions(args) = &cli.command {
        output::emit(&cli::completions(args.shell))?;
        return Ok(false);
    }
    let target = cli.target.path.as_deref();
    match cli.command {
        Command::Add(args) => commands::add::run_add(&bootstrap::services()?, target, args).await,
        Command::Import(args) => commands::add::run_import(target, args),
        Command::Remove(args) => commands::records::run_remove(target, args),
        Command::List(args) => commands::list::run(target, args),
        Command::Show(args) => commands::records::run_show(target, args),
        Command::Sync(args) => commands::sync::run(&bootstrap::services()?, target, args).await,
        Command::Check(args) => commands::check::run(target, args),
        Command::Init => commands::init::run(target),
        Command::Completions(_) => unreachable!(),
    }
}
