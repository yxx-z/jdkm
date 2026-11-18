mod cli;
mod commands;
mod error;
mod installed;
mod paths;
mod platform;
mod resolve;
mod version;

use std::process::ExitCode;

use anyhow::{Result, bail};
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
        Command::ListRemote { .. } => not_implemented("list-remote"),
        Command::Install { .. } => not_implemented("install"),
        Command::Uninstall { version } => commands::uninstall(&home, &version),
        Command::List => commands::list(&home),
        Command::Global { version } => commands::global(&home, &version),
        Command::Local { .. } => not_implemented("local"),
        Command::Shell { .. } => not_implemented("shell"),
        Command::Current => commands::current(&home),
        Command::Which { cmd } => commands::which(&home, &cmd),
        Command::Env => not_implemented("env"),
        Command::Init { .. } => not_implemented("init"),
    }
}

fn not_implemented(name: &str) -> Result<()> {
    bail!("`{name}` is not implemented yet")
}
