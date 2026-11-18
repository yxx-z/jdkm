//! JDK version numbers and user-supplied version requests.
//!
//! * [`Version`]  — a concrete version such as `21.0.5+11` (installed or remote).
//! * [`VersionRequest`] — what a user typed or a `.java-version` file holds,
//!   e.g. `21`, `21.0.5`, `temurin-17`; it may match many versions.

use std::cmp::Ordering;
use std::fmt;

use crate::error::JdkmError;

/// A concrete JDK version: dotted numbers plus an optional `+build` part.
/// `21+35`, `21.0.5+11` and `17.0.4.1+1` are all valid.
#[derive(Debug, Clone)]
pub struct Version {
    numbers: Vec<u32>,
    /// Empty when no `+build` part was given.
    build: Vec<u32>,
}

impl Version {
    pub fn parse(s: &str) -> Result<Self, JdkmError> {
        let invalid = || JdkmError::InvalidVersion(s.to_string());
        let (numbers, build) = match s.split_once('+') {
            Some((n, b)) => (n, Some(b)),
            None => (s, None),
        };
        let numbers = parse_dotted(numbers).ok_or_else(invalid)?;
        let build = match build {
            Some(b) => parse_dotted(b).ok_or_else(invalid)?,
            None => Vec::new(),
        };
        Ok(Self { numbers, build })
    }

    pub fn has_build(&self) -> bool {
        !self.build.is_empty()
    }

    pub fn major(&self) -> u32 {
        self.numbers[0] // parse() guarantees at least one number
    }

    /// Version as it appears in Temurin asset names: `21.0.5+11` -> `21.0.5_11`.
    pub fn underscored(&self) -> String {
        self.to_string().replace('+', "_")
    }
}

/// Parse `"21.0.5"` into `[21, 0, 5]`. Returns `None` on any bad component.
fn parse_dotted(s: &str) -> Option<Vec<u32>> {
    s.split('.').map(|part| part.parse::<u32>().ok()).collect()
}

/// Compare two number lists, treating missing trailing parts as 0,
/// so `21` == `21.0.0` and `21.0.5` < `21.0.10`.
fn cmp_padded(a: &[u32], b: &[u32]) -> Ordering {
    let len = a.len().max(b.len());
    (0..len)
        .map(|i| {
            let x = a.get(i).copied().unwrap_or(0);
            let y = b.get(i).copied().unwrap_or(0);
            x.cmp(&y)
        })
        .find(|ord| ord.is_ne())
        .unwrap_or(Ordering::Equal)
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        cmp_padded(&self.numbers, &other.numbers)
            .then_with(|| cmp_padded(&self.build, &other.build))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// Equality must agree with Ord (`21` == `21.0.0`), so it is not derived.
impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", join(&self.numbers))?;
        if self.has_build() {
            write!(f, "+{}", join(&self.build))?;
        }
        Ok(())
    }
}

fn join(parts: &[u32]) -> String {
    parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// A version as requested by the user, possibly partial and possibly with a
/// distribution prefix (`temurin-17`, jenv style).
#[derive(Debug, Clone)]
pub struct VersionRequest {
    /// Distribution prefix, if any. Currently informational only: matching
    /// ignores it (see DECISIONS.md #9).
    pub dist: Option<String>,
    version: Version,
}

impl VersionRequest {
    pub fn parse(s: &str) -> Result<Self, JdkmError> {
        let s = s.trim();
        // `temurin-17` -> ("temurin", "17"). Split at the last '-' so a
        // prefix that itself contains '-' still works.
        let (dist, version_part) = match s.rsplit_once('-') {
            Some((prefix, rest)) if !prefix.is_empty() => (Some(prefix.to_string()), rest),
            _ => (None, s),
        };
        let version =
            Version::parse(version_part).map_err(|_| JdkmError::InvalidVersion(s.to_string()))?;
        Ok(Self { dist, version })
    }

    /// Does this request select `candidate`? Every number the user gave must
    /// match; parts they left out match anything. `21.0` matches `21.0.5+11`.
    pub fn matches(&self, candidate: &Version) -> bool {
        let numbers_match = self
            .version
            .numbers
            .iter()
            .enumerate()
            .all(|(i, n)| candidate.numbers.get(i).copied().unwrap_or(0) == *n);
        let build_match =
            !self.version.has_build() || cmp_padded(&self.version.build, &candidate.build).is_eq();
        numbers_match && build_match
    }

    /// The version part without the distribution prefix, e.g. `17`.
    pub fn version_str(&self) -> String {
        self.version.to_string()
    }

    pub fn major(&self) -> u32 {
        self.version.major()
    }

    pub fn not_installed(&self) -> JdkmError {
        JdkmError::NotInstalled {
            version: self.version_str(),
        }
    }
}

impl fmt::Display for VersionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(dist) = &self.dist {
            write!(f, "{dist}-")?;
        }
        write!(f, "{}", self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn req(s: &str) -> VersionRequest {
        VersionRequest::parse(s).unwrap()
    }

    #[test]
    fn parses_and_displays() {
        for s in ["21", "21.0.5", "21.0.5+11", "17.0.4.1+1", "21+35"] {
            assert_eq!(v(s).to_string(), s);
        }
    }

    #[test]
    fn rejects_garbage() {
        for s in ["", "abc", "21.x", "21+", "+11", "21..5", "-1"] {
            assert!(Version::parse(s).is_err(), "{s:?} should be invalid");
        }
    }

    #[test]
    fn orders_numerically_with_padding() {
        assert!(v("21.0.10+7") > v("21.0.5+11"));
        assert!(v("21.0.5+11") > v("21.0.5+9"));
        assert!(v("21.0.1+12") > v("21+35"));
        assert!(v("17.0.4.1+1") > v("17.0.4+8"));
        assert_eq!(v("21"), v("21.0.0"));
    }

    #[test]
    fn request_strips_distribution_prefix() {
        let r = req("temurin-17");
        assert_eq!(r.dist.as_deref(), Some("temurin"));
        assert_eq!(r.version_str(), "17");

        let r = req("  21.0.5 \n");
        assert_eq!(r.dist, None);
        assert_eq!(r.version_str(), "21.0.5");

        assert!(VersionRequest::parse("temurin-").is_err());
        assert!(VersionRequest::parse("-17").is_err());
    }

    #[test]
    fn request_matching() {
        let installed = v("21.0.5+11");
        assert!(req("21").matches(&installed));
        assert!(req("21.0").matches(&installed));
        assert!(req("21.0.5").matches(&installed));
        assert!(req("21.0.5+11").matches(&installed));
        assert!(req("temurin-21").matches(&installed));

        assert!(!req("17").matches(&installed));
        assert!(!req("21.0.4").matches(&installed));
        assert!(!req("21.0.5+9").matches(&installed));
        assert!(!req("2").matches(&installed));
    }
}
