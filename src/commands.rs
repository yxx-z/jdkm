//! Implementations of the subcommands. Each function does the user-facing
//! part (printing); the logic lives in the library-like modules.

use std::fs;

use anyhow::{Context, Result, bail};

use crate::cli::ShellKind;
use crate::env::{self, Action, ShellState};
use crate::error::JdkmError;
use crate::installed;
use crate::paths::JdkmHome;
use crate::resolve::{self, Inputs, VERSION_ENV, VERSION_FILE};
use crate::shell;
use crate::version::VersionRequest;

pub fn list(home: &JdkmHome) -> Result<()> {
    let all = installed::list_installed(home)?;
    if all.is_empty() {
        println!("No JDKs installed. Run: jdkm install <version>");
        return Ok(());
    }

    // A configured-but-missing version should not stop us from listing.
    let active = match resolve::active(home, &Inputs::from_process()) {
        Ok(active) => active,
        Err(err) if is_not_installed(&err) => {
            eprintln!("warning: {err:#}");
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

pub fn local(home: &JdkmHome, version: &str) -> Result<()> {
    let request = VersionRequest::parse(version)?;
    let jdk = installed::require(home, &request)?;
    // Store what the user asked for (e.g. "21"), not the resolved build,
    // so the file stays portable across machines (DECISIONS.md #7).
    let path = std::env::current_dir()
        .context("cannot determine current directory")?
        .join(VERSION_FILE);
    fs::write(&path, format!("{request}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    println!(
        "Wrote {request} to {} (currently {})",
        path.display(),
        jdk.name
    );
    Ok(())
}

pub fn shell(home: &JdkmHome, version: Option<&str>, unset: bool) -> Result<()> {
    // Only the shell function can change the calling shell's environment.
    // It sets this variable and evals our stdout.
    let Some(kind) = std::env::var(shell::WRAPPER_VAR).ok() else {
        bail!(
            "`jdkm shell` needs the shell integration. Add this to ~/.zshrc and restart the shell:\n  \
             eval \"$(jdkm init zsh)\""
        );
    };
    let sh = match kind.as_str() {
        "zsh" => shell::for_kind(ShellKind::Zsh),
        other => bail!("unsupported shell `{other}` in {}", shell::WRAPPER_VAR),
    };

    // Messages go to stderr; stdout is reserved for code the wrapper evals.
    if unset {
        print!("{}", sh.render_all(&[Action::Unset(VERSION_ENV)]));
        eprintln!("Shell override removed");
        return Ok(());
    }
    let version = version.context("missing version")?; // clap enforces this
    let request = VersionRequest::parse(version)?;
    let jdk = installed::require(home, &request)?;
    print!(
        "{}",
        sh.render_all(&[Action::Set(VERSION_ENV, request.to_string())])
    );
    eprintln!("Using {} in this shell", jdk.name);
    Ok(())
}

pub fn current(home: &JdkmHome) -> Result<()> {
    let inputs = Inputs::from_process();
    let active = resolve::active(home, &inputs)?.ok_or(JdkmError::NoVersionSet)?;
    println!("{} ({})", active.jdk.name, active.source);

    // DECISIONS.md #9: a foreign distribution prefix is accepted but ignored.
    if let Some(sel) = resolve::select(home, &inputs)?
        && let Some(dist) = sel.request.dist.as_deref()
        && dist != active.jdk.dist
    {
        eprintln!(
            "note: `{}` asks for distribution `{dist}`; only {} is available, so it is used instead",
            sel.request, active.jdk.dist
        );
    }
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

    // Only the global default matters here, not the current directory.
    let was_default = match installed::read_default(home)? {
        Some(raw) => VersionRequest::parse(&raw)
            .ok()
            .and_then(|req| installed::find_best(&all, &req).map(|d| d.name == jdk.name))
            .unwrap_or(false),
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
    let active = resolve::active(home, &Inputs::from_process())?.ok_or(JdkmError::NoVersionSet)?;
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

/// Called before every prompt. Never fails loudly: problems are printed once
/// (see `env::plan`) and the prompt carries on.
pub fn env(home: &JdkmHome, kind: ShellKind) -> Result<()> {
    let path = match std::env::var("PATH") {
        Ok(p) => p,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(std::env::VarError::NotUnicode(_)) => bail!("PATH is not valid UTF-8; not touching it"),
    };
    let state = ShellState {
        path,
        java_home: std::env::var("JAVA_HOME").ok(),
        last_error: std::env::var(env::LAST_ERROR_VAR).ok(),
    };

    let resolved = resolve::active(home, &Inputs::from_process());
    let target = match &resolved {
        Ok(active) => Ok(active.as_ref().map(|a| &a.jdk)),
        Err(err) => Err(format!("{err:#}")),
    };

    let plan = env::plan(home, target, &state);
    if let Some(msg) = &plan.message {
        eprintln!("jdkm: {msg}");
    }
    print!("{}", shell::for_kind(kind).render_all(&plan.actions));
    Ok(())
}

pub fn init(kind: ShellKind) -> Result<()> {
    let exe = std::env::current_exe().context("cannot locate the jdkm executable")?;
    print!("{}", shell::for_kind(kind).init_script(&exe));
    Ok(())
}

fn is_not_installed(err: &anyhow::Error) -> bool {
    matches!(
        err.downcast_ref::<JdkmError>(),
        Some(JdkmError::NotInstalled { .. })
    )
}
