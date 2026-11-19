//! Eclipse Temurin from GitHub Releases.
//!
//! Each major version has its own repository, `adoptium/temurin<N>-binaries`.
//! Observed API facts this code relies on (checked 2026-11-19):
//! * The releases list is huge (~39 MB per 100 releases: 300+ assets each)
//!   and is NOT sorted by version, so we list lightweight *tags* instead and
//!   sort them ourselves.
//! * GA tags look like `jdk-21.0.5+11`, `jdk-21+35` or `jdk-21.0.13.1+2`.
//!   Early-access tags (`jdk-21.0.8+6-ea-beta`) and alias tags
//!   (`jdk-21.0.12-ga`) do not parse as versions and are skipped.
//! * Assets are named `OpenJDK21U-jdk_aarch64_mac_hotspot_21.0.5_11.tar.gz`,
//!   with a `<asset>.sha256.txt` next to it: "<hex>  <file name>".

use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use super::cache::{self, DEFAULT_TTL};
use super::http::{self, Http};
use super::{Artifact, Distribution, DownloadSource, MIN_MAJOR, Release};
use crate::platform::{Arch, Os};
use crate::version::Version;

const API: &str = "https://api.github.com";
const ORG: &str = "adoptium";
const ADOPTIUM: &str = "https://api.adoptium.net/v3";
const TUNA: &str = "https://mirrors.tuna.tsinghua.edu.cn/Adoptium";
/// Safety net against endless pagination.
const MAX_PAGES: usize = 10;

pub struct Temurin<'a> {
    http: &'a Http,
    cache_dir: PathBuf,
    os: Os,
    arch: Arch,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct GhRelease {
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    /// GitHub-computed `sha256:<hex>`; missing on older assets.
    digest: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseNames {
    releases: Vec<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    release_name: String,
    binaries: Vec<ApiBinary>,
}

#[derive(Deserialize)]
struct ApiBinary {
    package: ApiPackage,
}

#[derive(Deserialize)]
struct ApiPackage {
    name: String,
    link: String,
    size: u64,
    checksum: String,
}

fn parse_checksum(text: &str) -> Result<String> {
    let word = text.split_whitespace().next().unwrap_or_default();
    ensure!(
        word.len() == 64 && word.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "expected a SHA-256 hex digest"
    );
    Ok(word.to_ascii_lowercase())
}

impl<'a> Temurin<'a> {
    pub fn new(http: &'a Http, cache_dir: PathBuf) -> Result<Self> {
        Ok(Self {
            http,
            cache_dir,
            os: Os::current()?,
            arch: Arch::current()?,
        })
    }

    /// Fetch every page of a GitHub list endpoint and collect the `name`s.
    fn all_names(&self, first_url: String) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut url = Some(first_url);
        for _ in 0..MAX_PAGES {
            let Some(current) = url.take() else { break };
            let (page, next): (Vec<Named>, _) = self.http.github_json(&current)?;
            names.extend(page.into_iter().map(|n| n.name));
            url = next;
        }
        Ok(names)
    }

    fn asset_name(&self, version: &Version) -> String {
        format!(
            "OpenJDK{}U-jdk_{}_{}_hotspot_{}.tar.gz",
            version.major(),
            self.arch.temurin_name(),
            self.os.temurin_name(),
            version.underscored()
        )
    }

    fn api_tags(&self, major: u32) -> Result<Vec<String>> {
        let end = u64::from(major) + 1;
        let mut tags = Vec::new();
        for page in 0..MAX_PAGES {
            let url = format!(
                "{ADOPTIUM}/info/release_names?version=%5B{major},{end}%29&release_type=ga&vendor=eclipse&project=jdk&page_size=20&page={page}"
            );
            let result: ReleaseNames = match self.http.json(&url) {
                Err(err) if page > 0 && http::is_not_found(&err) => break,
                other => other?,
            };
            let count = result.releases.len();
            tags.extend(result.releases);
            if count < 20 {
                break;
            }
        }
        Ok(tags)
    }

    fn build_artifact(
        &self,
        release: &Release,
        url: String,
        size: u64,
        sha256: String,
    ) -> Artifact {
        let file_name = self.asset_name(&release.version);
        let mirror = format!(
            "{TUNA}/{}/jdk/{}/{}/{}",
            release.version.major(),
            self.arch.temurin_name(),
            self.os.temurin_name(),
            file_name
        );
        Artifact {
            file_name,
            size,
            sha256,
            sources: [
                DownloadSource {
                    name: "GitHub",
                    url,
                },
                DownloadSource {
                    name: "TUNA",
                    url: mirror,
                },
            ],
        }
    }

    fn api_artifact(&self, release: &Release, tag: &str) -> Result<Option<Artifact>> {
        let url = format!(
            "{ADOPTIUM}/assets/release_name/eclipse/{tag}?architecture={}&os={}&image_type=jdk&jvm_impl=hotspot",
            self.arch.temurin_name(),
            self.os.temurin_name()
        );
        let result: ApiRelease = match self.http.json(&url) {
            Err(err) if http::is_not_found(&err) => return Ok(None),
            other => other?,
        };
        self.match_package(release, result)
    }

    fn match_package(&self, release: &Release, result: ApiRelease) -> Result<Option<Artifact>> {
        ensure!(
            result.release_name == release.tag,
            "Adoptium returned a different release"
        );
        let wanted = self.asset_name(&release.version);
        result
            .binaries
            .into_iter()
            .map(|binary| binary.package)
            .find(|package| package.name == wanted)
            .map(|package| {
                Ok(self.build_artifact(
                    release,
                    package.link,
                    package.size,
                    parse_checksum(&package.checksum)?,
                ))
            })
            .transpose()
    }
}

impl Distribution for Temurin<'_> {
    fn name(&self) -> &'static str {
        "temurin"
    }

    fn majors(&self, refresh: bool) -> Result<Vec<u32>> {
        let path = self.cache_dir.join("temurin-majors.json");
        let repos = cache::cached(&path, DEFAULT_TTL, refresh, || {
            self.all_names(format!("{API}/orgs/{ORG}/repos?per_page=100&type=public"))
        })?;
        let mut majors: Vec<u32> = repos.iter().filter_map(|r| parse_repo_name(r)).collect();
        majors.retain(|m| *m >= MIN_MAJOR);
        majors.sort_unstable();
        majors.dedup();
        Ok(majors)
    }

    fn releases(&self, major: u32, refresh: bool) -> Result<Vec<Release>> {
        if major < MIN_MAJOR {
            bail!("JDK {major} is not supported; jdkm supports JDK {MIN_MAJOR} and newer");
        }
        let path = self.cache_dir.join(format!("temurin-{major}-tags.json"));
        let tags = cache::cached(&path, DEFAULT_TTL, refresh, || {
            let url = format!("{API}/repos/{ORG}/temurin{major}-binaries/tags?per_page=100");
            match self.all_names(url) {
                Ok(tags) => Ok(tags),
                Err(err) if http::is_not_found(&err) => {
                    bail!("Temurin {major} is not available. Run: jdkm list-remote")
                }
                Err(err) => self
                    .api_tags(major)
                    .with_context(|| format!("GitHub tag lookup also failed: {err:#}")),
            }
        })?;

        let mut releases: Vec<Release> = tags
            .into_iter()
            .filter_map(|tag| {
                let version = parse_tag(&tag)?;
                (version.major() == major).then_some(Release { version, tag })
            })
            .collect();
        releases.sort_by(|a, b| b.version.cmp(&a.version)); // newest first
        releases.dedup_by(|a, b| a.version == b.version);
        Ok(releases)
    }

    fn artifact(&self, release: &Release) -> Result<Option<Artifact>> {
        let major = release.version.major();
        // '+' must be escaped in a URL path segment.
        let tag = release.tag.replace('+', "%2B");
        let url = format!("{API}/repos/{ORG}/temurin{major}-binaries/releases/tags/{tag}");
        let gh: GhRelease = match self.http.github_json_opt(&url) {
            Ok(Some(gh)) => gh,
            Ok(None) => return Ok(None),
            Err(err) => {
                return self
                    .api_artifact(release, &tag)
                    .with_context(|| format!("GitHub release lookup also failed: {err:#}"));
            }
        };

        let wanted = self.asset_name(&release.version);
        let checksum_name = format!("{wanted}.sha256.txt");
        let archive = gh.assets.iter().find(|a| a.name == wanted);
        let checksum = gh.assets.iter().find(|a| a.name == checksum_name);
        match (archive, checksum) {
            (Some(archive), Some(checksum)) => {
                let expected = match self.http.get_text(&checksum.browser_download_url) {
                    Ok(text) => parse_checksum(&text)?,
                    Err(err) => {
                        self.api_artifact(release, &tag)?
                            .context(format!("no official checksum available: {err:#}"))?
                            .sha256
                    }
                };
                if let Some(digest) = archive
                    .digest
                    .as_deref()
                    .and_then(|d| d.strip_prefix("sha256:"))
                {
                    ensure!(
                        expected.eq_ignore_ascii_case(digest),
                        "official checksum sources disagree for {wanted}; refusing to install"
                    );
                }
                Ok(Some(self.build_artifact(
                    release,
                    archive.browser_download_url.clone(),
                    archive.size,
                    expected,
                )))
            }
            (Some(_), None) => {
                bail!("{wanted} has no published SHA-256 checksum; refusing to install")
            }
            (None, _) => Ok(None),
        }
    }
}

/// `temurin21-binaries` -> 21
fn parse_repo_name(name: &str) -> Option<u32> {
    name.strip_prefix("temurin")?
        .strip_suffix("-binaries")?
        .parse()
        .ok()
}

/// `jdk-21.0.5+11` -> 21.0.5+11. GA releases always carry a build number.
fn parse_tag(tag: &str) -> Option<Version> {
    let version = Version::parse(tag.strip_prefix("jdk-")?).ok()?;
    version.has_build().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_official_checksum() {
        let hex = "55a40abeb0e174fdc70f769b34b50b70c3967e0b12a643e6a3e23f9a582aac16";
        assert_eq!(
            parse_checksum(&format!("{hex}  jdk.tar.gz\n")).unwrap(),
            hex
        );
        assert_eq!(parse_checksum(&hex.to_uppercase()).unwrap(), hex);
        assert!(parse_checksum("").is_err());
        assert!(parse_checksum("abc file").is_err());
    }

    #[test]
    fn both_sources_are_locked_to_one_official_build_and_platform() {
        let http = Http::new().unwrap();
        let temurin = Temurin {
            http: &http,
            cache_dir: PathBuf::new(),
            os: Os::Mac,
            arch: Arch::Aarch64,
        };
        let release = Release {
            version: Version::parse("21.0.13+9").unwrap(),
            tag: "jdk-21.0.13+9".into(),
        };
        let wanted = "OpenJDK21U-jdk_aarch64_mac_hotspot_21.0.13_9.tar.gz";
        let checksum = "a".repeat(64);
        let response = |tag: &str, name: &str| ApiRelease {
            release_name: tag.into(),
            binaries: vec![ApiBinary {
                package: ApiPackage {
                    name: name.into(),
                    link: format!(
                        "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.13%2B9/{name}"
                    ),
                    size: 123,
                    checksum: checksum.clone(),
                },
            }],
        };
        assert!(
            temurin
                .match_package(&release, response("jdk-21.0.12+2", wanted))
                .is_err()
        );
        assert!(
            temurin
                .match_package(
                    &release,
                    response(&release.tag, &wanted.replace("aarch64", "x64"))
                )
                .unwrap()
                .is_none()
        );
        let artifact = temurin
            .match_package(&release, response(&release.tag, wanted))
            .unwrap()
            .unwrap();
        assert_eq!(artifact.sha256, checksum);
        assert_eq!(artifact.size, 123);
        assert_eq!(
            artifact.sources[1].url,
            format!("{TUNA}/21/jdk/aarch64/mac/{wanted}")
        );
        assert!(artifact.sources[0].url.ends_with(wanted));
    }

    #[test]
    fn repo_names() {
        assert_eq!(parse_repo_name("temurin21-binaries"), Some(21));
        assert_eq!(parse_repo_name("temurin8-binaries"), Some(8));
        assert_eq!(parse_repo_name("temurin-build"), None);
        assert_eq!(parse_repo_name("temurin21-binaries-old"), None);
    }

    #[test]
    fn tags() {
        let ok = |t: &str| parse_tag(t).map(|v| v.to_string());
        assert_eq!(ok("jdk-21.0.5+11").as_deref(), Some("21.0.5+11"));
        assert_eq!(ok("jdk-21+35").as_deref(), Some("21+35"));
        assert_eq!(ok("jdk-21.0.13.1+2").as_deref(), Some("21.0.13.1+2"));
        assert_eq!(ok("jdk-21.0.8+6-ea-beta"), None);
        assert_eq!(ok("jdk-21.0.12-ga"), None);
        assert_eq!(ok("jdk8u432-b06"), None);
    }

    #[test]
    fn asset_names() {
        let http = Http::new().unwrap();
        let t = Temurin {
            http: &http,
            cache_dir: PathBuf::new(),
            os: Os::Mac,
            arch: Arch::Aarch64,
        };
        let v = Version::parse("21.0.13.1+2").unwrap();
        assert_eq!(
            t.asset_name(&v),
            "OpenJDK21U-jdk_aarch64_mac_hotspot_21.0.13.1_2.tar.gz"
        );
        let t = Temurin {
            os: Os::Linux,
            arch: Arch::X64,
            ..t
        };
        let v = Version::parse("17.0.13+11").unwrap();
        assert_eq!(
            t.asset_name(&v),
            "OpenJDK17U-jdk_x64_linux_hotspot_17.0.13_11.tar.gz"
        );
    }
}
