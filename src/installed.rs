//! Locally installed JDKs (`$JDKM_HOME/versions/*`) and the global default
//! (`$JDKM_HOME/default` + the `current` symlink).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::paths::JdkmHome;
use crate::platform;
use crate::version::{Version, VersionRequest};

/// One directory under `versions/`, named `<dist>-<version>`,
/// e.g. `temurin-21.0.5+11`. The directory itself is the JAVA_HOME.
#[derive(Debug, Clone)]
pub struct Installed {
    /// Directory name, used as the user-facing identifier.
    pub name: String,
    pub dist: String,
    pub version: Version,
    /// Absolute path of the directory, i.e. JAVA_HOME.
    pub path: PathBuf,
}

impl Installed {
    pub fn bin_dir(&self) -> PathBuf {
        self.path.join("bin")
    }
}

/// Split `temurin-21.0.5+11` into (`temurin`, version).
fn parse_dir_name(name: &str) -> Option<(String, Version)> {
    let (dist, version) = name.split_once('-')?;
    if dist.is_empty() {
        return None;
    }
    Some((dist.to_string(), Version::parse(version).ok()?))
}

/// All installed JDKs, sorted from oldest to newest. Entries that do not
/// look like `<dist>-<version>` directories are ignored.
pub fn list_installed(home: &JdkmHome) -> Result<Vec<Installed>> {
    let dir = home.versions_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", dir.display())),
    };

    let mut result = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        // Skip names that are not valid UTF-8 or not in our naming scheme.
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((dist, version)) = parse_dir_name(&name) else {
            continue;
        };
        result.push(Installed {
            name,
            dist,
            version,
            path: entry.path(),
        });
    }
    result.sort_by(|a, b| a.version.cmp(&b.version).then_with(|| a.dist.cmp(&b.dist)));
    Ok(result)
}

/// The newest installed JDK matching `request`, if any.
pub fn find_best<'a>(
    installed: &'a [Installed],
    request: &VersionRequest,
) -> Option<&'a Installed> {
    installed
        .iter()
        .filter(|jdk| request.matches(&jdk.version))
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// Like [`find_best`] but turns "nothing matches" into a `NotInstalled` error.
pub fn require(home: &JdkmHome, request: &VersionRequest) -> Result<Installed> {
    let installed = list_installed(home)?;
    match find_best(&installed, request) {
        Some(jdk) => Ok(jdk.clone()),
        None => Err(request.not_installed().into()),
    }
}

/// Contents of the `default` file, or `None` if no global default is set.
pub fn read_default(home: &JdkmHome) -> Result<Option<String>> {
    let path = home.default_file();
    match fs::read_to_string(&path) {
        Ok(s) if s.trim().is_empty() => Ok(None),
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Make `jdk` the global default: write `default` and repoint `current`.
pub fn set_default(home: &JdkmHome, jdk: &Installed) -> Result<()> {
    write_atomic(&home.default_file(), &format!("{}\n", jdk.name))?;

    // Build the new symlink under a temporary name, then rename it over the
    // old one, so `current` is never missing or half-updated.
    let link = home.current_link();
    let tmp_link = home.root().join(".current.tmp");
    remove_if_exists(&tmp_link)?;
    let target = Path::new("versions").join(&jdk.name); // relative: survives moving JDKM_HOME
    platform::symlink_dir(&target, &tmp_link)
        .with_context(|| format!("failed to create symlink {}", tmp_link.display()))?;
    fs::rename(&tmp_link, &link).with_context(|| format!("failed to update {}", link.display()))?;
    Ok(())
}

/// Remove the global default (both `default` and `current`).
pub fn clear_default(home: &JdkmHome) -> Result<()> {
    remove_if_exists(&home.default_file())?;
    remove_if_exists(&home.current_link())?;
    Ok(())
}

/// Delete an installed JDK. The directory is first renamed into `tmp/` so
/// that an interrupted delete never leaves a half-removed version visible.
pub fn remove(home: &JdkmHome, jdk: &Installed) -> Result<()> {
    let trash = home
        .tmp_dir()
        .join(format!("uninstall-{}-{}", jdk.name, std::process::id()));
    fs::rename(&jdk.path, &trash)
        .with_context(|| format!("failed to move {}", jdk.path.display()))?;
    fs::remove_dir_all(&trash).with_context(|| format!("failed to delete {}", trash.display()))?;
    Ok(())
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("failed to remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Create a fake JDK directory with an executable `bin/java`.
    pub(crate) fn fake_jdk(home: &JdkmHome, name: &str) {
        let bin = home.versions_dir().join(name).join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("java"), "#!/bin/sh\n").unwrap();
    }

    pub(crate) fn temp_home() -> (tempfile::TempDir, JdkmHome) {
        let tmp = tempfile::tempdir().unwrap();
        let home = JdkmHome::new(tmp.path().join("jdkm"));
        home.ensure_layout().unwrap();
        (tmp, home)
    }

    fn names(list: &[Installed]) -> Vec<&str> {
        list.iter().map(|j| j.name.as_str()).collect()
    }

    #[test]
    fn lists_sorted_and_skips_junk() {
        let (_tmp, home) = temp_home();
        fake_jdk(&home, "temurin-21.0.5+11");
        fake_jdk(&home, "temurin-17.0.13+11");
        fake_jdk(&home, "temurin-21.0.10+7");
        fake_jdk(&home, "not-a-version");
        fs::write(
            home.versions_dir().join("temurin-11.0.1+1"),
            "a file, not a dir",
        )
        .unwrap();

        let list = list_installed(&home).unwrap();
        assert_eq!(
            names(&list),
            [
                "temurin-17.0.13+11",
                "temurin-21.0.5+11",
                "temurin-21.0.10+7"
            ]
        );
    }

    #[test]
    fn missing_versions_dir_means_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let home = JdkmHome::new(tmp.path().join("nope"));
        assert!(list_installed(&home).unwrap().is_empty());
    }

    #[test]
    fn find_best_picks_newest_match() {
        let (_tmp, home) = temp_home();
        fake_jdk(&home, "temurin-21.0.5+11");
        fake_jdk(&home, "temurin-21.0.10+7");
        fake_jdk(&home, "temurin-17.0.13+11");
        let list = list_installed(&home).unwrap();

        let pick =
            |s| find_best(&list, &VersionRequest::parse(s).unwrap()).map(|j| j.name.as_str());
        assert_eq!(pick("21"), Some("temurin-21.0.10+7"));
        assert_eq!(pick("21.0.5"), Some("temurin-21.0.5+11"));
        assert_eq!(pick("17"), Some("temurin-17.0.13+11"));
        assert_eq!(pick("11"), None);
    }

    #[test]
    fn require_reports_not_installed() {
        let (_tmp, home) = temp_home();
        let err = require(&home, &VersionRequest::parse("17").unwrap()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "JDK 17 is not installed. Run: jdkm install 17"
        );
    }

    #[test]
    fn set_and_clear_default() {
        let (_tmp, home) = temp_home();
        fake_jdk(&home, "temurin-17.0.13+11");
        fake_jdk(&home, "temurin-21.0.5+11");
        let list = list_installed(&home).unwrap();

        assert_eq!(read_default(&home).unwrap(), None);

        set_default(&home, &list[0]).unwrap();
        set_default(&home, &list[1]).unwrap(); // overwriting must work too
        assert_eq!(
            read_default(&home).unwrap().as_deref(),
            Some("temurin-21.0.5+11")
        );
        let link = fs::read_link(home.current_link()).unwrap();
        assert_eq!(link, Path::new("versions/temurin-21.0.5+11"));
        assert!(home.current_link().join("bin/java").is_file());

        clear_default(&home).unwrap();
        assert_eq!(read_default(&home).unwrap(), None);
        assert!(fs::symlink_metadata(home.current_link()).is_err());
        clear_default(&home).unwrap(); // idempotent
    }

    #[test]
    fn remove_deletes_directory() {
        let (_tmp, home) = temp_home();
        fake_jdk(&home, "temurin-17.0.13+11");
        let jdk = list_installed(&home).unwrap().remove(0);
        remove(&home, &jdk).unwrap();
        assert!(list_installed(&home).unwrap().is_empty());
        assert_eq!(fs::read_dir(home.tmp_dir()).unwrap().count(), 0);
    }
}
