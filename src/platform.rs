//! Platform-specific operations live here so the rest of the code stays
//! portable. Only macOS (and other Unix) is supported for now.

use std::io;
use std::path::Path;

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
