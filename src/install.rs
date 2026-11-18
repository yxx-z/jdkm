//! Downloading, verifying and unpacking a JDK into `versions/`.
//!
//! Atomicity: everything happens under `$JDKM_HOME/tmp/` (same filesystem as
//! `versions/`), and the finished JAVA_HOME is moved into place with a single
//! `rename`. Until that rename succeeds, `versions/` is untouched; temporary
//! files are removed by [`TempPath`] guards even when a step fails.

use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

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

    let expected = fetch_checksum(http, &artifact.checksum_url)?;
    if let Some(github) = &artifact.github_sha256
        && *github != expected
    {
        bail!(
            "checksum sources disagree for {}: {} says {expected}, GitHub says {github}; refusing to install",
            artifact.file_name,
            artifact.checksum_url
        );
    }
    let actual = download(http, artifact, archive.path())?;
    if actual != expected {
        // `archive` is dropped on return, deleting the bad file.
        bail!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}. The download was deleted; please retry.",
            artifact.file_name
        );
    }
    eprintln!("SHA-256 verified");

    install_archive(home, archive.path(), name, os, &tag)
    // `archive` is dropped here: the tarball is deleted (DECISIONS.md #4).
}

/// The checksum file's first word, validated as 64 hex digits.
fn fetch_checksum(http: &Http, url: &str) -> Result<String> {
    let text = http.get_text(url)?;
    parse_checksum(&text).with_context(|| format!("malformed checksum file {url}"))
}

fn parse_checksum(text: &str) -> Result<String> {
    let word = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if word.len() != 64 || !word.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("expected a SHA-256 hex digest, got `{word}`");
    }
    Ok(word)
}

/// Stream the archive to `dest` with a progress bar, hashing as we go.
/// Returns the lowercase hex SHA-256 of what was written.
fn download(http: &Http, artifact: &Artifact, dest: &Path) -> Result<String> {
    let mut resp = http.get_stream(&artifact.url)?;
    let total = resp.content_length().unwrap_or(artifact.size);

    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template(
            "{msg} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec} ETA {eta}",
        )?
        .progress_chars("=> "),
    );
    bar.set_message("Downloading");

    let mut file =
        File::create(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    loop {
        let n = resp.read(&mut buf).context("download interrupted")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .with_context(|| format!("failed to write {}", dest.display()))?;
        written += n as u64;
        bar.inc(n as u64);
    }
    file.sync_all()?;
    bar.finish_and_clear();

    if artifact.size > 0 && written != artifact.size {
        bail!(
            "download incomplete: got {written} of {} bytes",
            artifact.size
        );
    }
    eprintln!(
        "Downloaded {} ({:.1} MB)",
        artifact.file_name,
        written as f64 / 1e6
    );
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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

    fs::rename(&java_home, &target)
        .with_context(|| format!("failed to move JDK into {}", target.display()))?;
    Ok(target)
    // `staging` (what is left of the archive, e.g. Contents/Info.plist) is deleted here.
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
    fn parses_checksum_files() {
        let hex = "55a40abeb0e174fdc70f769b34b50b70c3967e0b12a643e6a3e23f9a582aac16";
        assert_eq!(
            parse_checksum(&format!("{hex}  OpenJDK21U.tar.gz\n")).unwrap(),
            hex
        );
        assert_eq!(parse_checksum(&hex.to_uppercase()).unwrap(), hex);
        assert!(parse_checksum("").is_err());
        assert!(parse_checksum("abc file").is_err());
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

    #[test]
    fn sha256_hex() {
        assert_eq!(
            to_hex(&Sha256::digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
