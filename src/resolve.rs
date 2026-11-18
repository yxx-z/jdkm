//! Which JDK should be active, and why.
//!
//! Milestone 2 only knows the global default. Milestone 3 adds the
//! `JDKM_VERSION` variable and `.java-version` files in front of it.

use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::installed::{self, Installed};
use crate::paths::JdkmHome;
use crate::version::VersionRequest;

/// Where the version choice came from.
#[derive(Debug, Clone)]
pub enum Source {
    /// `$JDKM_HOME/default`
    Global(PathBuf),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Global(path) => write!(f, "set by {}", path.display()),
        }
    }
}

/// The requested version and its source, before checking it is installed.
#[derive(Debug, Clone)]
pub struct Selection {
    pub request: VersionRequest,
    pub source: Source,
}

/// An installed JDK that is active, together with why.
#[derive(Debug, Clone)]
pub struct Active {
    pub jdk: Installed,
    pub source: Source,
}

/// Find the requested version, without touching `versions/`.
pub fn select(home: &JdkmHome) -> Result<Option<Selection>> {
    let Some(raw) = installed::read_default(home)? else {
        return Ok(None);
    };
    let path = home.default_file();
    let request = VersionRequest::parse(&raw)
        .with_context(|| format!("bad version in {}", path.display()))?;
    Ok(Some(Selection {
        request,
        source: Source::Global(path),
    }))
}

/// Find the active JDK. `Ok(None)` means nothing is configured; a configured
/// but missing version is an error (`JdkmError::NotInstalled`).
pub fn active(home: &JdkmHome) -> Result<Option<Active>> {
    let Some(selection) = select(home)? else {
        return Ok(None);
    };
    let jdk = installed::require(home, &selection.request)?;
    Ok(Some(Active {
        jdk,
        source: selection.source,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed::tests::{fake_jdk, temp_home};

    #[test]
    fn nothing_configured() {
        let (_tmp, home) = temp_home();
        assert!(active(&home).unwrap().is_none());
    }

    #[test]
    fn global_default_is_used() {
        let (_tmp, home) = temp_home();
        fake_jdk(&home, "temurin-21.0.5+11");
        let jdk = installed::list_installed(&home).unwrap().remove(0);
        installed::set_default(&home, &jdk).unwrap();

        let active = active(&home).unwrap().unwrap();
        assert_eq!(active.jdk.name, "temurin-21.0.5+11");
        assert!(matches!(active.source, Source::Global(_)));
    }

    #[test]
    fn global_default_not_installed_is_an_error() {
        let (_tmp, home) = temp_home();
        std::fs::write(home.default_file(), "temurin-17.0.13+11\n").unwrap();
        let err = active(&home).unwrap_err();
        assert_eq!(
            err.to_string(),
            "JDK 17.0.13+11 is not installed. Run: jdkm install 17.0.13+11"
        );
    }
}
