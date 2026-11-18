mod cli;
mod paths;

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
        Command::Uninstall { .. } => not_implemented("uninstall"),
        Command::List => not_implemented("list"),
        Command::Global { .. } => not_implemented("global"),
        Command::Local { .. } => not_implemented("local"),
        Command::Shell { .. } => not_implemented("shell"),
        Command::Current => not_implemented("current"),
        Command::Which { .. } => not_implemented("which"),
        Command::Env => not_implemented("env"),
        Command::Init { .. } => not_implemented("init"),
    }
}

fn not_implemented(name: &str) -> Result<()> {
    bail!("`{name}` is not implemented yet")
}
