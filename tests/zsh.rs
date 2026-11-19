use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A temp dir with a jdkm home holding fake JDKs 17 and 21, plus
/// `project-17` / `project-21` directories pinned to them.
fn workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let home = root.join("jdkm home");
    for version in ["17", "21"] {
        let bin = home.join(format!("versions/temurin-{version}/bin"));
        fs::create_dir_all(&bin).unwrap();
        let project = root.join(format!("project-{version}"));
        fs::create_dir(&project).unwrap();
        fs::write(project.join(".java-version"), version).unwrap();
    }
    (temp, root, home)
}

/// Copy the jdkm binary under test to `dir/jdkm`.
fn copy_binary(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let dest = dir.join("jdkm");
    fs::copy(env!("CARGO_BIN_EXE_jdkm"), &dest).unwrap();
    dest
}

/// Run `script` in an interactive zsh (so precmd/chpwd hooks fire).
fn zsh(root: &Path, home: &Path, path: &str, jdkm_bin: &Path, script: &str) -> Output {
    let mut shell = Command::new("zsh")
        .args(["-dfi"])
        .current_dir(root)
        .env_clear()
        .env("PATH", path)
        .env("HOME", root)
        .env("JDKM_HOME", home)
        .env("JDKM_BIN", jdkm_bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zsh is required to test shell integration");
    shell
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = shell.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Lines printed by the fake `mvn test` below: "test: <JAVA_HOME> <PATH[1]>".
fn mvn_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with("test:"))
        .map(String::from)
        .collect()
}

const FAKE_MVN: &str = r#"mvn() { print -r -- "$1: ${JAVA_HOME:t} ${path[1]:h:t}"; }"#;

#[test]
fn switches_before_same_line_command_and_before_next_prompt() {
    let (_temp, root, home) = workspace();
    let output = zsh(
        &root,
        &home,
        "/usr/bin:/bin",
        Path::new(env!("CARGO_BIN_EXE_jdkm")),
        &format!(
            r#"
eval "$("$JDKM_BIN" init zsh)"
eval "$("$JDKM_BIN" init zsh)"
print -r -- "hooks: ${{#chpwd_functions}} ${{#precmd_functions}}"
{FAKE_MVN}
cd project-17
mvn test
cd ../project-21 && mvn test
jdkm local 17
mvn test
exit
"#
        ),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.lines().any(|line| line == "hooks: 1 1"), "{stdout}");
    assert_eq!(
        mvn_lines(&output),
        [
            "test: temurin-17 temurin-17",
            "test: temurin-21 temurin-21",
            "test: temurin-17 temurin-17"
        ]
    );
}

#[test]
fn finds_jdkm_on_path_after_the_original_binary_moves() {
    let (_temp, root, home) = workspace();
    let old = copy_binary(&root.join("old"));
    let new = copy_binary(&root.join("new"));
    let path = format!("{}:/usr/bin:/bin", new.parent().unwrap().display());
    let output = zsh(
        &root,
        &home,
        &path,
        &old,
        &format!(
            r#"
eval "$("$JDKM_BIN" init zsh)"
rm "$JDKM_BIN"
{FAKE_MVN}
cd project-17 && mvn test
print -r -- "bin: $_jdkm_bin"
jdkm --version >/dev/null && print -r -- "wrapper ok"
exit
"#
        ),
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(mvn_lines(&output), ["test: temurin-17 temurin-17"]);
    assert!(
        stdout.contains(&format!("bin: {}", new.display())),
        "{stdout}"
    );
    assert!(stdout.contains("wrapper ok"), "{stdout}");
    assert!(!stderr.contains("not found"), "{stderr}");
}

#[test]
fn a_removed_binary_is_reported_once_and_the_hook_turns_itself_off() {
    let (_temp, root, home) = workspace();
    let old = copy_binary(&root.join("old"));
    let output = zsh(
        &root,
        &home,
        "/usr/bin:/bin",
        &old,
        r#"
eval "$("$JDKM_BIN" init zsh)"
rm "$JDKM_BIN"
cd project-17
true
true
cd ../project-21
print -r -- "hooks left: ${#chpwd_functions} ${#precmd_functions}"
jdkm list; print -r -- "wrapper exit: $?"
exit
"#,
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let notices = stderr.matches("JDK switching is off").count();
    assert_eq!(notices, 1, "{stderr}");
    assert!(stdout.contains("hooks left: 0 0"), "{stdout}");
    assert!(stdout.contains("wrapper exit: 127"), "{stdout}");
}
