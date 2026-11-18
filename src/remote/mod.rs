//! Remote JDK sources. Each distribution (Temurin today; Zulu, Corretto
//! later) implements [`Distribution`].

pub mod cache;
pub mod http;
pub mod temurin;

use anyhow::Result;

use crate::version::Version;

/// Oldest major version we support (DECISIONS.md #5).
pub const MIN_MAJOR: u32 = 11;

/// A published release, identified by its version.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    /// Source-specific identifier, e.g. the git tag `jdk-21.0.5+11`.
    pub tag: String,
}

/// The downloadable archive of a release for the current platform.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub file_name: String,
    pub url: String,
    /// Size in bytes as reported by the source (0 if unknown).
    pub size: u64,
    /// URL of a text file whose first word is the hex SHA-256 of the archive.
    pub checksum_url: String,
    /// An independent SHA-256 from the hosting service, if it provides one
    /// (GitHub's asset `digest`). Cross-checked against `checksum_url`.
    pub github_sha256: Option<String>,
}

pub trait Distribution {
    /// Short lowercase name, used as the `versions/<name>-<version>` prefix.
    fn name(&self) -> &'static str;

    /// Major versions that can be installed, ascending.
    fn majors(&self, refresh: bool) -> Result<Vec<u32>>;

    /// General-availability releases of one major version, newest first.
    fn releases(&self, major: u32, refresh: bool) -> Result<Vec<Release>>;

    /// The archive for this machine's OS/CPU, or `None` if there is no build.
    fn artifact(&self, release: &Release) -> Result<Option<Artifact>>;
}

/// Long-term-support releases: 8, 11, then every fourth release from 17.
pub fn is_lts(major: u32) -> bool {
    major == 8 || major == 11 || (major >= 17 && (major - 17).is_multiple_of(4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lts_rule() {
        let lts: Vec<u32> = (8..=30).filter(|m| is_lts(*m)).collect();
        assert_eq!(lts, [8, 11, 17, 21, 25, 29]);
    }
}
