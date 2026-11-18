//! Planning the environment changes `jdkm env` should make.
//!
//! This runs before every prompt, so it must be fast, must never touch the
//! network, and must be idempotent: applying its output and running it again
//! yields no further changes.

use crate::installed::Installed;
use crate::paths::JdkmHome;

/// Remembers the last problem we reported, so a missing JDK is reported
/// once instead of before every prompt (DECISIONS.md #12).
pub const LAST_ERROR_VAR: &str = "_JDKM_LAST_ERROR";

/// A single environment change, rendered into shell syntax by a `Shell`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Set(&'static str, String),
    Unset(&'static str),
}

/// The parts of the shell environment `jdkm env` cares about.
#[derive(Debug, Clone, Default)]
pub struct ShellState {
    pub path: String,
    pub java_home: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub actions: Vec<Action>,
    /// A problem to show the user (on stderr), if not shown already.
    pub message: Option<String>,
}

/// What resolution decided: a JDK, nothing configured, or a problem
/// (already formatted for the user).
pub type Target<'a> = Result<Option<&'a Installed>, String>;

pub fn plan(home: &JdkmHome, target: Target<'_>, state: &ShellState) -> Plan {
    let versions_dir = home.versions_dir().to_string_lossy().into_owned();
    let mut actions = Vec::new();

    // PATH: drop every `$JDKM_HOME/versions/*/bin`, then prepend the new one.
    let mut entries: Vec<String> = if state.path.is_empty() {
        Vec::new() // splitting "" would yield one empty entry, which means "."
    } else {
        state
            .path
            .split(':')
            .filter(|entry| !is_jdkm_bin(entry, &versions_dir))
            .map(String::from)
            .collect()
    };

    let desired_java_home = match &target {
        Ok(Some(jdk)) => {
            entries.insert(0, jdk.bin_dir().to_string_lossy().into_owned());
            Some(jdk.path.to_string_lossy().into_owned())
        }
        // Nothing to use: only clean up what jdkm itself set, and leave a
        // JAVA_HOME pointing elsewhere alone (DECISIONS.md #11).
        Ok(None) | Err(_) => state
            .java_home
            .clone()
            .filter(|jh| !is_under(jh, &versions_dir)),
    };

    let new_path = entries.join(":");
    if new_path != state.path {
        actions.push(Action::Set("PATH", new_path));
    }

    match (&desired_java_home, &state.java_home) {
        (Some(want), Some(have)) if want == have => {}
        (Some(want), _) => actions.push(Action::Set("JAVA_HOME", want.clone())),
        (None, Some(_)) => actions.push(Action::Unset("JAVA_HOME")),
        (None, None) => {}
    }

    // Report each distinct problem once.
    let mut message = None;
    match target {
        Err(msg) => {
            if state.last_error.as_deref() != Some(msg.as_str()) {
                actions.push(Action::Set(LAST_ERROR_VAR, msg.clone()));
                message = Some(msg);
            }
        }
        Ok(_) => {
            if state.last_error.is_some() {
                actions.push(Action::Unset(LAST_ERROR_VAR));
            }
        }
    }

    Plan { actions, message }
}

/// Is `entry` exactly `<versions_dir>/<one component>/bin`?
fn is_jdkm_bin(entry: &str, versions_dir: &str) -> bool {
    let entry = entry.trim_end_matches('/');
    entry
        .strip_prefix(versions_dir)
        .and_then(|rest| rest.strip_prefix('/'))
        .and_then(|rest| rest.strip_suffix("/bin"))
        .is_some_and(|name| !name.is_empty() && !name.contains('/'))
}

/// Is `path` inside `dir` (or `dir` itself)?
fn is_under(path: &str, dir: &str) -> bool {
    path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed::list_installed;
    use crate::installed::tests::{fake_jdk, temp_home};

    /// Apply a plan to a simulated shell, like `eval` would.
    fn apply(state: &mut ShellState, plan: &Plan) {
        for action in &plan.actions {
            match action {
                Action::Set("PATH", v) => state.path = v.clone(),
                Action::Set("JAVA_HOME", v) => state.java_home = Some(v.clone()),
                Action::Set(LAST_ERROR_VAR, v) => state.last_error = Some(v.clone()),
                Action::Unset("JAVA_HOME") => state.java_home = None,
                Action::Unset(LAST_ERROR_VAR) => state.last_error = None,
                other => panic!("unexpected action {other:?}"),
            }
        }
    }

    fn setup() -> (tempfile::TempDir, JdkmHome, Vec<Installed>, String) {
        let (tmp, home) = temp_home();
        fake_jdk(&home, "temurin-17.0.13+11");
        fake_jdk(&home, "temurin-21.0.5+11");
        let jdks = list_installed(&home).unwrap();
        let v = home.versions_dir().to_string_lossy().into_owned();
        (tmp, home, jdks, v)
    }

    #[test]
    fn activates_jdk_and_is_idempotent() {
        let (_tmp, home, jdks, v) = setup();
        let mut state = ShellState {
            path: "/usr/bin:/bin".into(),
            ..Default::default()
        };

        let first = plan(&home, Ok(Some(&jdks[0])), &state);
        apply(&mut state, &first);
        assert_eq!(
            state.path,
            format!("{v}/temurin-17.0.13+11/bin:/usr/bin:/bin")
        );
        assert_eq!(state.java_home, Some(format!("{v}/temurin-17.0.13+11")));

        // Running again with the same target changes nothing and prints nothing.
        assert_eq!(plan(&home, Ok(Some(&jdks[0])), &state), Plan::default());
    }

    #[test]
    fn switching_replaces_old_entry_without_growing_path() {
        let (_tmp, home, jdks, v) = setup();
        let mut state = ShellState {
            path: "/usr/bin".into(),
            ..Default::default()
        };
        for _ in 0..3 {
            for jdk in &jdks {
                let p = plan(&home, Ok(Some(jdk)), &state);
                apply(&mut state, &p);
            }
        }
        assert_eq!(state.path, format!("{v}/temurin-21.0.5+11/bin:/usr/bin"));
    }

    #[test]
    fn removes_stray_entries_anywhere_in_path() {
        let (_tmp, home, jdks, v) = setup();
        let state = ShellState {
            path: format!("/a:{v}/temurin-21.0.5+11/bin:/b:{v}/old-1/bin/:/c"),
            ..Default::default()
        };
        let p = plan(&home, Ok(Some(&jdks[0])), &state);
        let Action::Set("PATH", path) = &p.actions[0] else {
            panic!("{p:?}")
        };
        assert_eq!(path, &format!("{v}/temurin-17.0.13+11/bin:/a:/b:/c"));
    }

    #[test]
    fn keeps_unrelated_entries_that_look_similar() {
        let v = "/h/.jdkm/versions";
        assert!(is_jdkm_bin("/h/.jdkm/versions/x-1/bin", v));
        assert!(!is_jdkm_bin("/h/.jdkm/versions/x-1/bin/extra", v));
        assert!(!is_jdkm_bin("/h/.jdkm/versions/a/b/bin", v));
        assert!(!is_jdkm_bin("/h/.jdkm/versions2/x-1/bin", v));
        assert!(!is_jdkm_bin("/h/.jdkm/current/bin", v));
    }

    #[test]
    fn nothing_configured_cleans_up_only_our_own_state() {
        let (_tmp, home, _jdks, v) = setup();
        let mut state = ShellState {
            path: format!("{v}/temurin-17.0.13+11/bin:/usr/bin"),
            java_home: Some(format!("{v}/temurin-17.0.13+11")),
            ..Default::default()
        };
        let p = plan(&home, Ok(None), &state);
        apply(&mut state, &p);
        assert_eq!(state.path, "/usr/bin");
        assert_eq!(state.java_home, None);

        // A JAVA_HOME the user set themselves is left alone.
        let state = ShellState {
            path: "/usr/bin".into(),
            java_home: Some("/Library/Java/foo".into()),
            ..Default::default()
        };
        assert_eq!(plan(&home, Ok(None), &state), Plan::default());
    }

    #[test]
    fn errors_are_reported_once_and_cleared_later() {
        let (_tmp, home, jdks, _v) = setup();
        let mut state = ShellState {
            path: "/usr/bin".into(),
            ..Default::default()
        };
        let err = || Err("JDK 11 is not installed".to_string());

        let p = plan(&home, err(), &state);
        assert_eq!(p.message.as_deref(), Some("JDK 11 is not installed"));
        apply(&mut state, &p);

        // Same problem at the next prompt: silent, no changes.
        assert_eq!(plan(&home, err(), &state), Plan::default());

        // Problem gone: the marker is cleared, so it would be reported again later.
        let p = plan(&home, Ok(Some(&jdks[0])), &state);
        apply(&mut state, &p);
        assert_eq!(state.last_error, None);
        assert!(plan(&home, err(), &state).message.is_some());
    }

    #[test]
    fn empty_path_does_not_gain_a_dot_entry() {
        let (_tmp, home, jdks, v) = setup();
        let state = ShellState::default();
        let p = plan(&home, Ok(Some(&jdks[0])), &state);
        assert_eq!(
            p.actions[0],
            Action::Set("PATH", format!("{v}/temurin-17.0.13+11/bin"))
        );
    }
}
