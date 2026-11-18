//! Command-line interface definition (clap derive style).

use clap::{Parser, Subcommand, ValueEnum};

/// A JDK version manager: install Temurin JDKs from GitHub Releases and
/// switch between them globally, per directory, or per shell session.
#[derive(Debug, Parser)]
#[command(name = "jdkm", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List JDK versions available for installation
    ListRemote {
        /// Only show versions of this major release (e.g. 21)
        #[arg(long)]
        major: Option<u32>,
        /// Ignore the cached release list and query the remote source again
        #[arg(long)]
        refresh: bool,
    },

    /// Install a JDK (e.g. `21`, `17`, `21.0.5+11`)
    Install {
        /// Version to install; a bare major version picks the latest release
        version: String,
    },

    /// Uninstall an installed JDK
    Uninstall {
        /// Installed version to remove
        version: String,
    },

    /// List installed JDKs and mark the active one
    List,

    /// Set the global default JDK
    Global {
        /// Installed version to use by default
        version: String,
    },

    /// Write a .java-version file in the current directory
    Local {
        /// Version to pin for this directory and its subdirectories
        version: String,
    },

    /// Use a JDK in the current shell session only (requires the shell hook)
    Shell {
        /// Version to use in this shell
        #[arg(required_unless_present = "unset", conflicts_with = "unset")]
        version: Option<String>,
        /// Stop overriding the version in this shell
        #[arg(long)]
        unset: bool,
    },

    /// Show the active JDK and where that choice comes from
    Current,

    /// Show the real path of a JDK command such as `java` or `javac`
    Which {
        /// Command name, e.g. java
        cmd: String,
    },

    /// Print export statements for the current directory (used by the shell hook)
    Env {
        /// Shell syntax to print
        #[arg(long, value_enum, default_value_t = ShellKind::Zsh)]
        shell: ShellKind,
    },

    /// Print the shell hook script to add to your shell config
    Init {
        /// Target shell
        shell: ShellKind,
    },
}

/// Shells supported by `jdkm init`. Add bash/fish here later.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ShellKind {
    Zsh,
}

impl Command {
    /// Whether this command only reads state. Read-only commands must not
    /// create anything under JDKM_HOME (`env` in particular runs on every prompt).
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Command::List
                | Command::Current
                | Command::Which { .. }
                | Command::Env { .. }
                | Command::Init { .. }
                | Command::Shell { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // Catches conflicting/duplicated argument definitions at test time.
        Cli::command().debug_assert();
    }

    #[test]
    fn shell_requires_version_or_unset() {
        assert!(Cli::try_parse_from(["jdkm", "shell"]).is_err());
        assert!(Cli::try_parse_from(["jdkm", "shell", "17"]).is_ok());
        assert!(Cli::try_parse_from(["jdkm", "shell", "--unset"]).is_ok());
        assert!(Cli::try_parse_from(["jdkm", "shell", "17", "--unset"]).is_err());
    }
}
