//! Implementations of the subcommands. Each function does the user-facing
//! part (printing); the logic lives in the library-like modules.

use anyhow::{Result, bail};

use crate::error::JdkmError;
use crate::installed;
use crate::paths::JdkmHome;
use crate::resolve;
use crate::version::VersionRequest;

pub fn list(home: &JdkmHome) -> Result<()> {
    let all = installed::list_installed(home)?;
    if all.is_empty() {
        println!("No JDKs installed. Run: jdkm install <version>");
        return Ok(());
    }

    // A configured-but-missing version should not stop us from listing.
    let active = match resolve::active(home) {
        Ok(active) => active,
        Err(err) if is_not_installed(&err) => {
            eprintln!("warning: {err}");
            None
        }
        Err(err) => return Err(err),
    };

    for jdk in &all {
        match &active {
            Some(a) if a.jdk.name == jdk.name => println!("* {} ({})", jdk.name, a.source),
            _ => println!("  {}", jdk.name),
        }
    }
    Ok(())
}

pub fn global(home: &JdkmHome, version: &str) -> Result<()> {
    let request = VersionRequest::parse(version)?;
    let jdk = installed::require(home, &request)?;
    installed::set_default(home, &jdk)?;
    println!("Global default set to {}", jdk.name);
    Ok(())
}

pub fn current(home: &JdkmHome) -> Result<()> {
    let active = resolve::active(home)?.ok_or(JdkmError::NoVersionSet)?;
    println!("{} ({})", active.jdk.name, active.source);
    Ok(())
}

pub fn uninstall(home: &JdkmHome, version: &str) -> Result<()> {
    let request = VersionRequest::parse(version)?;
    let all = installed::list_installed(home)?;
    let matching: Vec<_> = all
        .iter()
        .filter(|jdk| request.matches(&jdk.version))
        .collect();

    // Deleting is destructive, so a request must name exactly one JDK.
    let jdk = match matching.as_slice() {
        [] => return Err(request.not_installed().into()),
        [only] => *only,
        many => {
            let names: Vec<_> = many.iter().map(|j| j.name.as_str()).collect();
            bail!(
                "`{version}` matches several installed JDKs: {}. Specify one, e.g. jdkm uninstall {}",
                names.join(", "),
                many[0].name
            );
        }
    };

    let was_default = match resolve::select(home)? {
        Some(sel) => installed::find_best(&all, &sel.request).is_some_and(|d| d.name == jdk.name),
        None => false,
    };

    installed::remove(home, jdk)?;
    println!("Uninstalled {}", jdk.name);

    if was_default {
        installed::clear_default(home)?;
        eprintln!(
            "warning: {} was the global default; no global default is set now. Run: jdkm global <version>",
            jdk.name
        );
    }
    Ok(())
}

pub fn which(home: &JdkmHome, cmd: &str) -> Result<()> {
    let active = resolve::active(home)?.ok_or(JdkmError::NoVersionSet)?;
    let path = active.jdk.bin_dir().join(cmd);
    if !path.is_file() {
        bail!(
            "`{cmd}` not found in {} ({})",
            active.jdk.name,
            active.jdk.bin_dir().display()
        );
    }
    println!("{}", path.display());
    Ok(())
}

fn is_not_installed(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<JdkmError>(),
        Some(JdkmError::NotInstalled { .. })
    )
}
