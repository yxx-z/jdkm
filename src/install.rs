//! Downloading, verifying and unpacking a JDK into `versions/`.
//!
//! Atomicity: everything happens under `$JDKM_HOME/tmp/` (same filesystem as
//! `versions/`), and the finished JAVA_HOME is moved into place with a single
//! `rename`. Until that rename succeeds, `versions/` is untouched; temporary
//! files are removed by [`TempPath`] guards even when a step fails.

use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

use crate::download::{self, Tuning};
use crate::paths::JdkmHome;
use crate::platform::Os;
use crate::remote::Artifact;
use crate::remote::http::Http;

/// Deletes a file or directory when dropped — on success, on `?` early
/// return, or on panic. Rust's version of try/finally cleanup (RAII).
pub struct TempPath {
    path: PathBuf,
}

impl TempPath {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        // Errors are ignored: there is nothing sensible to do in a destructor.
        let _ = if self.path.is_dir() {
            fs::remove_dir_all(&self.path)
        } else {
            fs::remove_file(&self.path)
        };
    }
}

/// Download `artifact`, verify its SHA-256 and install it as `versions/<name>`.
/// Returns the new JAVA_HOME.
pub fn download_and_install(
    home: &JdkmHome,
    http: &Http,
    artifact: &Artifact,
    name: &str,
    os: Os,
) -> Result<PathBuf> {
    let tag = format!("{name}.{}", std::process::id());
    let archive = TempPath::new(home.tmp_dir().join(format!("{tag}.tar.gz")));

    download::download_verified(http, artifact, archive.path(), &Tuning::default())?;
    eprintln!("SHA-256 verified");

    install_archive(home, archive.path(), name, os, &tag)
    // `archive` is dropped here: the tarball is deleted (DECISIONS.md #4).
}

/// Unpack a verified `.tar.gz` and move its JAVA_HOME to `versions/<name>`.
pub fn install_archive(
    home: &JdkmHome,
    archive: &Path,
    name: &str,
    os: Os,
    tag: &str,
) -> Result<PathBuf> {
    let target = home.versions_dir().join(name);
    if target.exists() {
        bail!("{name} is already installed");
    }

    let staging = TempPath::new(home.tmp_dir().join(format!("{tag}.extract")));
    eprintln!("Extracting...");
    let file =
        File::open(archive).with_context(|| format!("failed to open {}", archive.display()))?;
    let mut tar = tar::Archive::new(GzDecoder::new(BufReader::new(file)));
    tar.set_preserve_permissions(true);
    // `unpack` refuses entries that would escape the destination (`..`, absolute paths).
    tar.unpack(staging.path())
        .with_context(|| format!("failed to extract {}", archive.display()))?;

    let top = single_top_dir(staging.path())?;
    let java_home = os.java_home_in(&top);
    if !java_home.join("bin").join("java").is_file() {
        bail!(
            "unexpected archive layout: {} has no bin/java",
            java_home
                .strip_prefix(staging.path())
                .unwrap_or(&java_home)
                .display()
        );
    }

    // Still in staging: a JDK that does not start never reaches versions/.
    let banner = verify_runs(&java_home)?;
    eprintln!("Verified: {banner}");

    fs::rename(&java_home, &target)
        .with_context(|| format!("failed to move JDK into {}", target.display()))?;
    Ok(target)
    // `staging` (what is left of the archive, e.g. Contents/Info.plist) is deleted here.
}

/// Run `bin/java -version` once and return the first line of its output.
///
/// Two purposes:
/// * prove the JDK actually starts on this machine;
/// * absorb the slow *first* launch on macOS (code-signature assessment,
///   cold disk cache). Otherwise the first `java` run happens right after
///   `jdkm local`, e.g. by the starship prompt, which times out at 500 ms.
///   Running it from the staging directory is fine: the rename into
///   `versions/` keeps the same files, so the warm-up carries over.
fn verify_runs(java_home: &Path) -> Result<String> {
    let java = java_home.join("bin").join("java");
    let output = Command::new(&java)
        .arg("-version")
        .output()
        .with_context(|| format!("failed to run {}", java.display()))?;
    // `java -version` prints to stderr.
    let text = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "the downloaded JDK does not run (`java -version` exited with {}):\n{}",
            output.status,
            text.trim()
        );
    }
    let first_line = text.lines().next().unwrap_or_default().trim();
    Ok(if first_line.is_empty() {
        "java starts".to_string()
    } else {
        first_line.to_string()
    })
}

/// JDK archives contain exactly one top-level directory, e.g. `jdk-21.0.5+11/`.
fn single_top_dir(dir: &Path) -> Result<PathBuf> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        // macOS may add AppleDouble files (`._foo`); ignore anything that is not a directory.
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    match dirs.len() {
        1 => Ok(dirs.remove(0)),
        n => bail!("unexpected archive layout: expected one top-level directory, found {n}"),
    }
}

/// Remove leftovers in `tmp/` from interrupted runs (e.g. Ctrl-C, where
/// destructors do not run). Only entries older than an hour are touched, so
/// a concurrently running install is left alone.
pub fn clean_stale_tmp(home: &JdkmHome) {
    let Ok(entries) = fs::read_dir(home.tmp_dir()) else {
        return;
    };
    let cutoff = SystemTime::now() - Duration::from_secs(60 * 60);
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t < cutoff);
        if stale {
            drop(TempPath::new(entry.path())); // dropping deletes it
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed::tests::temp_home;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    #[test]
    fn a_download_that_fails_verification_installs_nothing_and_leaves_nothing() {
        use crate::remote::DownloadSource;
        use crate::remote::test_server::{Reply, Server};
        use sha2::{Digest, Sha256};

        let server = Server::new(|req| match req.path.as_str() {
            "/GitHub" => Reply::file(req, b"evil"),
            _ => Reply::status(404),
        });
        let artifact = Artifact {
            file_name: "jdk.tar.gz".into(),
            sources: ["GitHub", "TUNA"]
                .into_iter()
                .map(|name| DownloadSource {
                    name,
                    url: format!("{}/{name}", server.url),
                    missing_note: None,
                })
                .collect(),
            size: 4,
            sha256: download::to_hex(&Sha256::digest(b"good")),
        };
        let (_temp, home) = temp_home();
        let http = Http::new().unwrap();
        let err = download_and_install(&home, &http, &artifact, "temurin-21", Os::Mac).unwrap_err();
        assert!(err.to_string().contains("SHA-256 mismatch"), "{err}");
        // GitHub alone produced the bad file, so it is not asked again.
        let github: Vec<_> = server
            .downloads()
            .into_iter()
            .filter(|(p, _)| p == "/GitHub")
            .collect();
        assert_eq!(github, [("/GitHub".into(), 0)]);
        assert_eq!(fs::read_dir(home.versions_dir()).unwrap().count(), 0);
        assert_eq!(fs::read_dir(home.tmp_dir()).unwrap().count(), 0);
    }

    /// Build a .tar.gz containing the given (path, contents) files.
    fn make_tar_gz(dest: &Path, files: &[(&str, &str)]) {
        let gz = GzEncoder::new(File::create(dest).unwrap(), Compression::fast());
        let mut builder = tar::Builder::new(gz);
        for (path, contents) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, path, contents.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn installs_mac_layout_as_contents_home() {
        let (tmp, home) = temp_home();
        let archive = tmp.path().join("jdk.tar.gz");
        make_tar_gz(
            &archive,
            &[
                ("jdk-21.0.5+11/Contents/Info.plist", "<plist/>"),
                ("jdk-21.0.5+11/Contents/Home/bin/java", "#!/bin/sh\n"),
                (
                    "jdk-21.0.5+11/Contents/Home/release",
                    "JAVA_VERSION=\"21.0.5\"\n",
                ),
            ],
        );

        let java_home =
            install_archive(&home, &archive, "temurin-21.0.5+11", Os::Mac, "t").unwrap();
        assert_eq!(java_home, home.versions_dir().join("temurin-21.0.5+11"));
        assert!(java_home.join("bin/java").is_file());
        assert!(java_home.join("release").is_file());
        assert!(!java_home.join("Contents").exists());

        // Executable bit survives extraction.
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(java_home.join("bin/java"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);

        // Staging area cleaned up.
        assert_eq!(fs::read_dir(home.tmp_dir()).unwrap().count(), 0);
    }

    #[test]
    fn rejects_bad_layout_without_touching_versions() {
        let (tmp, home) = temp_home();
        let archive = tmp.path().join("jdk.tar.gz");
        // A Linux-style layout fed to the macOS installer: no Contents/Home.
        make_tar_gz(&archive, &[("jdk-21.0.5+11/bin/java", "#!/bin/sh\n")]);

        let err = install_archive(&home, &archive, "temurin-21.0.5+11", Os::Mac, "t").unwrap_err();
        assert!(
            err.to_string().contains("unexpected archive layout"),
            "{err}"
        );
        assert_eq!(fs::read_dir(home.versions_dir()).unwrap().count(), 0);
        assert_eq!(fs::read_dir(home.tmp_dir()).unwrap().count(), 0);
    }

    #[test]
    fn verifies_java_runs_and_reports_version() {
        let (tmp, home) = temp_home();
        let archive = tmp.path().join("jdk.tar.gz");
        let java = "#!/bin/sh\necho 'openjdk version \"21.0.5\" 2024-10-15' >&2\n";
        make_tar_gz(&archive, &[("jdk-21.0.5+11/bin/java", java)]);

        let java_home =
            install_archive(&home, &archive, "temurin-21.0.5+11", Os::Linux, "t").unwrap();
        // The installed JDK runs, and the banner is the first line of `java -version`.
        assert_eq!(
            verify_runs(&java_home).unwrap(),
            r#"openjdk version "21.0.5" 2024-10-15"#
        );
    }

    #[test]
    fn broken_jdk_never_reaches_versions() {
        let (tmp, home) = temp_home();
        let archive = tmp.path().join("jdk.tar.gz");
        let java = "#!/bin/sh\necho 'dyld: missing symbol' >&2\nexit 3\n";
        make_tar_gz(&archive, &[("jdk-21.0.5+11/bin/java", java)]);

        let err =
            install_archive(&home, &archive, "temurin-21.0.5+11", Os::Linux, "t").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("does not run"), "{msg}");
        assert!(msg.contains("dyld: missing symbol"), "{msg}");
        assert_eq!(fs::read_dir(home.versions_dir()).unwrap().count(), 0);
        assert_eq!(fs::read_dir(home.tmp_dir()).unwrap().count(), 0);
    }

    #[test]
    fn refuses_to_overwrite_existing_version() {
        let (tmp, home) = temp_home();
        crate::installed::tests::fake_jdk(&home, "temurin-21.0.5+11");
        let archive = tmp.path().join("jdk.tar.gz");
        make_tar_gz(&archive, &[("jdk/bin/java", "x")]);
        let err =
            install_archive(&home, &archive, "temurin-21.0.5+11", Os::Linux, "t").unwrap_err();
        assert!(err.to_string().contains("already installed"));
    }

    #[test]
    fn temp_path_cleans_up_files_and_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        let dir = tmp.path().join("d/e");
        fs::write(&file, "x").unwrap();
        fs::create_dir_all(&dir).unwrap();
        drop(TempPath::new(file.clone()));
        drop(TempPath::new(tmp.path().join("d")));
        assert!(!file.exists());
        assert!(!dir.exists());
    }
}
