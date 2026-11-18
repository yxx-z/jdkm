//! Error types that callers need to tell apart (e.g. the shell hook treats
//! "not installed" differently from a broken config). Everything else uses
//! plain `anyhow` errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum JdkmError {
    /// `version` is the version part the user asked for, without any
    /// distribution prefix, so the hint can be copy-pasted as-is.
    #[error("JDK {version} is not installed. Run: jdkm install {version}")]
    NotInstalled { version: String },

    #[error("no JDK version is set. Run: jdkm global <version>")]
    NoVersionSet,

    #[error("invalid version `{0}` (expected something like 21, 21.0.5 or 21.0.5+11)")]
    InvalidVersion(String),
}
