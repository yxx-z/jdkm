//! Locations of everything jdkm stores on disk.
//!
//! ```text
//! $JDKM_HOME (default ~/.jdkm)
//! ├── versions/<dist>-<version>/   each one is a JAVA_HOME
//! ├── current -> versions/...      symlink to the global default (for IDEs)
//! ├── default                      global default version
//! ├── cache/                       downloads and remote release list cache
//! ├── tmp/                         staging area for atomic installs
//! └── config.toml
//! ```

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Environment variable that overrides the jdkm root directory.
pub const JDKM_HOME_VAR: &str = "JDKM_HOME";

/// The jdkm root directory and the paths derived from it.
#[derive(Debug, Clone)]
pub struct JdkmHome {
    root: PathBuf,
}

// TODO(milestone 2): remove once every path accessor is used by a command.
#[allow(dead_code)]
impl JdkmHome {
    /// Locate the root from the real process environment.
    pub fn from_env() -> Result<Self> {
        let root = resolve_root(
            std::env::var_os(JDKM_HOME_VAR),
            dirs::home_dir(),
            std::env::current_dir().ok(),
        )?;
        Ok(Self { root })
    }

    /// Use an explicit root directory (handy in tests).
    #[cfg(test)]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn current_link(&self) -> PathBuf {
        self.root.join("current")
    }

    pub fn default_file(&self) -> PathBuf {
        self.root.join("default")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Create the directory layout if it does not exist yet. Idempotent.
    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [self.versions_dir(), self.cache_dir(), self.tmp_dir()] {
            fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create directory {}", dir.display()))?;
        }
        Ok(())
    }
}

/// Decide the root directory from its inputs. Kept free of global state so
/// it can be unit-tested without touching real environment variables.
fn resolve_root(
    jdkm_home: Option<OsString>,
    home_dir: Option<PathBuf>,
    current_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    // An empty JDKM_HOME is treated as unset.
    if let Some(value) = jdkm_home.filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
        // Make a relative JDKM_HOME absolute, otherwise PATH entries we
        // export would break as soon as the user changes directory.
        let cwd =
            current_dir.context("cannot resolve relative JDKM_HOME: unknown current directory")?;
        return Ok(cwd.join(path));
    }
    match home_dir {
        Some(home) => Ok(home.join(".jdkm")),
        None => bail!("cannot determine home directory; set {JDKM_HOME_VAR}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_dot_jdkm_in_home() {
        let root = resolve_root(None, Some("/Users/me".into()), None).unwrap();
        assert_eq!(root, PathBuf::from("/Users/me/.jdkm"));
    }

    #[test]
    fn absolute_jdkm_home_wins() {
        let root = resolve_root(Some("/opt/jdkm".into()), Some("/Users/me".into()), None).unwrap();
        assert_eq!(root, PathBuf::from("/opt/jdkm"));
    }

    #[test]
    fn empty_jdkm_home_is_ignored() {
        let root = resolve_root(Some("".into()), Some("/Users/me".into()), None).unwrap();
        assert_eq!(root, PathBuf::from("/Users/me/.jdkm"));
    }

    #[test]
    fn relative_jdkm_home_is_made_absolute() {
        let root = resolve_root(Some("j".into()), None, Some("/work".into())).unwrap();
        assert_eq!(root, PathBuf::from("/work/j"));
    }

    #[test]
    fn no_home_and_no_override_is_an_error() {
        assert!(resolve_root(None, None, None).is_err());
    }

    #[test]
    fn ensure_layout_creates_dirs_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = JdkmHome::new(tmp.path().join("jdkm"));
        home.ensure_layout().unwrap();
        home.ensure_layout().unwrap();
        assert!(home.versions_dir().is_dir());
        assert!(home.cache_dir().is_dir());
        assert!(home.tmp_dir().is_dir());
    }
}
