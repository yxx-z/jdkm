use std::path::Path;

use super::{HOOK_VAR, Shell, WRAPPER_VAR, quote};
use crate::env::Action;

pub struct Zsh;

impl Shell for Zsh {
    fn render(&self, action: &Action) -> String {
        match action {
            Action::Set(name, value) => format!("export {name}={}", quote(value)),
            Action::Unset(name) => format!("unset {name}"),
        }
    }

    fn init_script(&self, exe: &Path) -> String {
        INIT_TEMPLATE
            .replace("__JDKM_EXE__", &quote(&exe.to_string_lossy()))
            .replace("__WRAPPER_VAR__", WRAPPER_VAR)
            .replace("__HOOK_VAR__", HOOK_VAR)
    }
}

// Notes on the script:
// * chpwd updates before the next command; precmd also catches in-place changes.
// * `jdkm shell ...` must change *this* shell's environment, which a child
//   process cannot do; the function evals the binary's output instead.
// * Re-sourcing .zshrc must not register the hook twice.
const INIT_TEMPLATE: &str = r#"# jdkm shell integration for zsh
# Add to ~/.zshrc:  eval "$(jdkm init zsh)"

_jdkm_bin=__JDKM_EXE__
# Lets `jdkm local/global` know the hook is active.
export __HOOK_VAR__=zsh

jdkm() {
  if [[ "$1" == "shell" ]]; then
    local _jdkm_out
    _jdkm_out="$(__WRAPPER_VAR__=zsh "$_jdkm_bin" "$@")" || return
    eval "$_jdkm_out"
    _jdkm_hook
  else
    "$_jdkm_bin" "$@"
  fi
}

_jdkm_hook() {
  eval "$("$_jdkm_bin" env --shell zsh)"
}

typeset -ga precmd_functions chpwd_functions
if (( ! ${chpwd_functions[(I)_jdkm_hook]} )); then
  chpwd_functions=(_jdkm_hook $chpwd_functions)
fi
if (( ! ${precmd_functions[(I)_jdkm_hook]} )); then
  precmd_functions=(_jdkm_hook $precmd_functions)
fi
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_actions() {
        assert_eq!(
            Zsh.render(&Action::Set("JAVA_HOME", "/a b".into())),
            "export JAVA_HOME='/a b'"
        );
        assert_eq!(Zsh.render(&Action::Unset("JAVA_HOME")), "unset JAVA_HOME");
    }

    #[test]
    fn init_script_embeds_quoted_binary_path() {
        let script = Zsh.init_script(Path::new("/Users/me/.cargo/bin/jdkm"));
        assert!(script.contains("_jdkm_bin='/Users/me/.cargo/bin/jdkm'"));
        assert!(script.contains("JDKM_SHELL=zsh"));
        assert!(script.contains("precmd_functions"));
        assert!(!script.contains("__"), "unreplaced placeholder");
    }
}
