use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn switches_before_same_line_command_and_before_next_prompt() {
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
    let mut shell = Command::new("zsh")
        .args(["-dfi"])
        .current_dir(&root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &root)
        .env("JDKM_HOME", &home)
        .env("JDKM_BIN", env!("CARGO_BIN_EXE_jdkm"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zsh is required to test shell integration");
    shell
        .stdin
        .take()
        .unwrap()
        .write_all(
            br#"
eval "$("$JDKM_BIN" init zsh)"
eval "$("$JDKM_BIN" init zsh)"
print -r -- "hooks: ${#chpwd_functions} ${#precmd_functions}"
mvn() { print -r -- "$1: ${JAVA_HOME:t} ${path[1]:h:t}"; }
cd project-17
mvn test
cd ../project-21 && mvn test
jdkm local 17
mvn test
exit
"#,
        )
        .unwrap();
    let output = shell.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.lines().any(|line| line == "hooks: 1 1"), "{stdout}");
    assert_eq!(
        stdout
            .lines()
            .filter(|line| line.starts_with("test:"))
            .collect::<Vec<_>>(),
        [
            "test: temurin-17 temurin-17",
            "test: temurin-21 temurin-21",
            "test: temurin-17 temurin-17"
        ]
    );
}
