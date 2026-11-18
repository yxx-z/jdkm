//! Which JDK should be active, and why.
//!
//! Priority, highest first:
//! 1. `JDKM_VERSION` environment variable (set by `jdkm shell`)
//! 2. the nearest `.java-version` file, searching upwards from the current directory
//! 3. the global default in `$JDKM_HOME/default`
//! 4. nothing

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::installed::{self, Installed};
use crate::paths::JdkmHome;
use crate::version::VersionRequest;

pub const VERSION_ENV: &str = "JDKM_VERSION";
pub const VERSION_FILE: &str = ".java-version";

/// Where the version choice came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// The `JDKM_VERSION` environment variable.
    Shell,
    /// A `.java-version` file.
    File(PathBuf),
    /// `$JDKM_HOME/default`
    Global(PathBuf),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Shell => write!(f, "set by {VERSION_ENV} via `jdkm shell`"),
            Source::File(path) | Source::Global(path) => write!(f, "set by {}", path.display()),
        }
    }
}

/// Everything resolution reads from the process. Passing it in explicitly
/// (instead of reading globals deep inside) keeps resolution unit-testable.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub env_version: Option<String>,
    /// `None` if the current directory is unknown (e.g. it was deleted).
    pub cwd: Option<PathBuf>,
}

impl Inputs {
    pub fn from_process() -> Self {
        Self {
            env_version: std::env::var(VERSION_ENV).ok(),
            cwd: std::env::current_dir().ok(),
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

/// Find the requested version, without looking at `versions/`.
pub fn select(home: &JdkmHome, inputs: &Inputs) -> Result<Option<Selection>> {
    // 1. JDKM_VERSION (an empty value counts as unset)
    if let Some(raw) = inputs
        .env_version
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        let request =
            VersionRequest::parse(raw).with_context(|| format!("bad version in {VERSION_ENV}"))?;
        return Ok(Some(Selection {
            request,
            source: Source::Shell,
        }));
    }

    // 2. nearest .java-version
    if let Some(cwd) = &inputs.cwd
        && let Some((path, raw)) = find_version_file(cwd)?
    {
        let request = VersionRequest::parse(&raw)
            .with_context(|| format!("bad version in {}", path.display()))?;
        return Ok(Some(Selection {
            request,
            source: Source::File(path),
        }));
    }

    // 3. global default
    if let Some(raw) = installed::read_default(home)? {
        let path = home.default_file();
        let request = VersionRequest::parse(&raw)
            .with_context(|| format!("bad version in {}", path.display()))?;
        return Ok(Some(Selection {
            request,
            source: Source::Global(path),
        }));
    }

    // 4. nothing
    Ok(None)
}

/// Find the active JDK. `Ok(None)` means nothing is configured; a configured
/// but missing version is an error (`JdkmError::NotInstalled`).
pub fn active(home: &JdkmHome, inputs: &Inputs) -> Result<Option<Active>> {
    let Some(selection) = select(home, inputs)? else {
        return Ok(None);
    };
    let Selection { request, source } = selection;
    // Say where the missing version was requested, e.g.
    // "version 11 set by ~/p/.java-version: JDK 11 is not installed. ..."
    let jdk = installed::require(home, &request)
        .with_context(|| format!("version {request} {source}"))?;
    Ok(Some(Active { jdk, source }))
}

/// Walk from `start` up to the filesystem root and return the first
/// non-blank `.java-version` file with its contents. Blank files are skipped.
pub fn find_version_file(start: &Path) -> Result<Option<(PathBuf, String)>> {
    for dir in start.ancestors() {
        let path = dir.join(VERSION_FILE);
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if !contents.trim().is_empty() {
            return Ok(Some((path, contents)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed::tests::{fake_jdk, temp_home};
    use std::fs;

    /// A workspace for resolution tests:
    ///
    /// ```text
    /// <tmp>/ws/
    /// ├── projA/.java-version    = "17"
    /// │   └── sub/dir/
    /// ├── projB/.java-version    = "temurin-21"
    /// │   └── nested/.java-version = "  21.0.5 \n"
    /// ├── blank/.java-version    = "   \n"     (ignored)
    /// └── other/
    /// ```
    struct Fixture {
        _tmp: tempfile::TempDir,
        home: JdkmHome,
        ws: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let (tmp, home) = temp_home();
            fake_jdk(&home, "temurin-17.0.13+11");
            fake_jdk(&home, "temurin-21.0.5+11");
            fake_jdk(&home, "temurin-21.0.10+7");

            let ws = tmp.path().join("ws");
            for dir in ["projA/sub/dir", "projB/nested", "blank", "other"] {
                fs::create_dir_all(ws.join(dir)).unwrap();
            }
            fs::write(ws.join("projA/.java-version"), "17").unwrap();
            fs::write(ws.join("projB/.java-version"), "temurin-21").unwrap();
            fs::write(ws.join("projB/nested/.java-version"), "  21.0.5 \n").unwrap();
            fs::write(ws.join("blank/.java-version"), "   \n").unwrap();
            Self {
                _tmp: tmp,
                home,
                ws,
            }
        }

        fn set_global(&self, name: &str) {
            let jdk = installed::list_installed(&self.home)
                .unwrap()
                .into_iter()
                .find(|j| j.name == name)
                .unwrap();
            installed::set_default(&self.home, &jdk).unwrap();
        }

        /// Resolve from `dir` (relative to the workspace) with an optional JDKM_VERSION.
        fn resolve(&self, dir: &str, env: Option<&str>) -> Result<Option<Active>> {
            let inputs = Inputs {
                env_version: env.map(String::from),
                cwd: Some(self.ws.join(dir)),
            };
            active(&self.home, &inputs)
        }

        fn name(&self, dir: &str, env: Option<&str>) -> Option<String> {
            self.resolve(dir, env).unwrap().map(|a| a.jdk.name)
        }
    }

    #[test]
    fn nothing_configured() {
        let f = Fixture::new();
        assert_eq!(f.name("other", None), None);
    }

    #[test]
    fn version_file_in_current_dir() {
        let f = Fixture::new();
        let a = f.resolve("projA", None).unwrap().unwrap();
        assert_eq!(a.jdk.name, "temurin-17.0.13+11");
        assert_eq!(a.source, Source::File(f.ws.join("projA/.java-version")));
    }

    #[test]
    fn version_file_is_inherited_from_ancestors() {
        let f = Fixture::new();
        let a = f.resolve("projA/sub/dir", None).unwrap().unwrap();
        assert_eq!(a.jdk.name, "temurin-17.0.13+11");
        assert_eq!(a.source, Source::File(f.ws.join("projA/.java-version")));
    }

    #[test]
    fn nearest_version_file_wins() {
        let f = Fixture::new();
        // projB says 21 (latest 21.x), projB/nested pins 21.0.5
        assert_eq!(f.name("projB", None).as_deref(), Some("temurin-21.0.10+7"));
        assert_eq!(
            f.name("projB/nested", None).as_deref(),
            Some("temurin-21.0.5+11")
        );
    }

    #[test]
    fn blank_version_file_is_skipped() {
        let f = Fixture::new();
        f.set_global("temurin-21.0.10+7");
        let a = f.resolve("blank", None).unwrap().unwrap();
        assert!(matches!(a.source, Source::Global(_)));
    }

    #[test]
    fn global_default_applies_outside_projects() {
        let f = Fixture::new();
        f.set_global("temurin-21.0.10+7");
        let a = f.resolve("other", None).unwrap().unwrap();
        assert_eq!(a.jdk.name, "temurin-21.0.10+7");
        assert_eq!(a.source, Source::Global(f.home.default_file()));
        // ...but a project file still beats it
        assert_eq!(f.name("projA", None).as_deref(), Some("temurin-17.0.13+11"));
    }

    #[test]
    fn env_var_beats_everything() {
        let f = Fixture::new();
        f.set_global("temurin-21.0.10+7");
        let a = f.resolve("projA/sub/dir", Some("21.0.5")).unwrap().unwrap();
        assert_eq!(a.jdk.name, "temurin-21.0.5+11");
        assert_eq!(a.source, Source::Shell);
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let f = Fixture::new();
        assert_eq!(
            f.name("projA", Some("")).as_deref(),
            Some("temurin-17.0.13+11")
        );
    }

    #[test]
    fn unknown_cwd_falls_back_to_global() {
        let f = Fixture::new();
        f.set_global("temurin-17.0.13+11");
        let inputs = Inputs {
            env_version: None,
            cwd: None,
        };
        let a = active(&f.home, &inputs).unwrap().unwrap();
        assert!(matches!(a.source, Source::Global(_)));
    }

    #[test]
    fn requested_but_not_installed_is_an_error() {
        let f = Fixture::new();
        fs::write(f.ws.join("other/.java-version"), "11\n").unwrap();
        let err = f.resolve("other", None).unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            format!(
                "version 11 set by {}: JDK 11 is not installed. Run: jdkm install 11",
                f.ws.join("other/.java-version").display()
            )
        );
        // The typed error is still reachable through the context.
        assert!(matches!(
            err.downcast_ref::<crate::error::JdkmError>(),
            Some(crate::error::JdkmError::NotInstalled { .. })
        ));

        let err = f.resolve("projA", Some("8")).unwrap_err();
        assert!(format!("{err:#}").ends_with("JDK 8 is not installed. Run: jdkm install 8"));
    }

    #[test]
    fn garbage_version_file_mentions_the_file() {
        let f = Fixture::new();
        fs::write(f.ws.join("other/.java-version"), "latest\n").unwrap();
        let err = f.resolve("other", None).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("other/.java-version"), "{msg}");
        assert!(msg.contains("invalid version `latest`"), "{msg}");
    }
}
