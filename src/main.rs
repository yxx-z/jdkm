mod cli;
mod commands;
mod download;
mod env;
mod error;
mod install;
mod installed;
mod paths;
mod platform;
mod remote;
mod resolve;
mod shell;
mod version;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, Command};
use paths::JdkmHome;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // `{:#}` prints the whole context chain on one line:
            // "failed to create directory ...: Permission denied"
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(command: Command) -> Result<()> {
    let home = JdkmHome::from_env()?;
    if !command.is_read_only() {
        home.ensure_layout()?;
    }

    match command {
        Command::ListRemote { major, refresh } => commands::list_remote(&home, major, refresh),
        Command::Install { version } => commands::install(&home, &version),
        Command::Uninstall { version } => commands::uninstall(&home, &version),
        Command::List => commands::list(&home),
        Command::Global { version } => commands::global(&home, &version),
        Command::Local { version } => commands::local(&home, &version),
        Command::Shell { version, unset } => commands::shell(&home, version.as_deref(), unset),
        Command::Current => commands::current(&home),
        Command::Which { cmd } => commands::which(&home, &cmd),
        Command::Env { shell } => commands::env(&home, shell),
        Command::Init { shell } => commands::init(shell),
    }
}
