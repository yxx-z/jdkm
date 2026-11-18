//! Platform-specific operations live here so the rest of the code stays
//! portable. Only macOS (and other Unix) is supported for now.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// Operating systems, named the way JDK vendors name them in file names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
}

/// CPU architectures, named the way JDK vendors name them in file names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Aarch64,
    X64,
}

impl Os {
    pub fn current() -> Result<Self> {
        match std::env::consts::OS {
            "macos" => Ok(Os::Mac),
            "linux" => Ok(Os::Linux),
            other => bail!("unsupported operating system: {other}"),
        }
    }

    /// Name used in Temurin asset file names.
    pub fn temurin_name(self) -> &'static str {
        match self {
            Os::Mac => "mac",
            Os::Linux => "linux",
        }
    }

    /// Where JAVA_HOME lives inside the single top-level directory of an
    /// extracted JDK archive.
    ///
    /// macOS archives are app-bundle shaped: `jdk-21.0.5+11/Contents/Home`.
    /// We install only `Contents/Home` (DECISIONS.md #1), so that every
    /// `versions/<name>` directory is itself a JAVA_HOME.
    pub fn java_home_in(self, top_dir: &Path) -> PathBuf {
        match self {
            Os::Mac => top_dir.join("Contents").join("Home"),
            Os::Linux => top_dir.to_path_buf(),
        }
    }
}

impl Arch {
    pub fn current() -> Result<Self> {
        match std::env::consts::ARCH {
            "aarch64" => Ok(Arch::Aarch64),
            "x86_64" => Ok(Arch::X64),
            other => bail!("unsupported CPU architecture: {other}"),
        }
    }

    /// Name used in Temurin asset file names.
    pub fn temurin_name(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64",
            Arch::X64 => "x64",
        }
    }
}

/// Create a symbolic link at `link` pointing to directory `target`.
#[cfg(unix)]
pub fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
pub fn symlink_dir(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symlinks are only supported on Unix for now",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_java_home_is_contents_home() {
        let top = Path::new("/x/jdk-21.0.5+11");
        assert_eq!(
            Os::Mac.java_home_in(top),
            Path::new("/x/jdk-21.0.5+11/Contents/Home")
        );
        assert_eq!(Os::Linux.java_home_in(top), top);
    }

    #[test]
    fn detects_this_machine() {
        assert!(Os::current().is_ok());
        assert!(Arch::current().is_ok());
    }
}
