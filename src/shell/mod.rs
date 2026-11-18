//! Shell-specific syntax. Each supported shell implements [`Shell`];
//! add bash/fish by adding a module and a `ShellKind` variant.

mod zsh;

use std::path::Path;

use crate::cli::ShellKind;
use crate::env::Action;

/// Environment variable the shell wrapper sets when it runs `jdkm shell`,
/// so the binary knows its output will be `eval`ed.
pub const WRAPPER_VAR: &str = "JDKM_SHELL";

/// Exported by the init script, so commands can tell whether the hook is
/// active in the user's shell and warn if it is not.
pub const HOOK_VAR: &str = "JDKM_HOOK";

pub trait Shell {
    /// Render one environment change as a shell statement.
    fn render(&self, action: &Action) -> String;

    /// The script printed by `jdkm init <shell>`. `exe` is the absolute
    /// path of the jdkm binary, so the hook does not depend on PATH.
    fn init_script(&self, exe: &Path) -> String;

    /// Render a list of changes, one statement per line.
    fn render_all(&self, actions: &[Action]) -> String {
        actions.iter().map(|a| self.render(a) + "\n").collect()
    }
}

pub fn for_kind(kind: ShellKind) -> &'static dyn Shell {
    match kind {
        ShellKind::Zsh => &zsh::Zsh,
    }
}

/// Quote `s` as a single POSIX shell word: `it's` -> `'it'\''s'`.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("a b$c"), "'a b$c'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }
}
